#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod claude;
mod coach;
mod hotkeys;
mod html;
mod knowledge;
mod lua;
mod models;
mod screen;
mod secret;
mod selftest;
mod settings;
mod stt;
mod text;
mod tts;
mod tts_edge;
mod wow;

use coach::{Coach, Paths};
use serde_json::{json, Value};
use settings::Settings;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Manager, State};

type CoachState<'a> = State<'a, Arc<Coach>>;

const APP_IDENTIFIER: &str = "fr.mentor.app";
/// Data folder of the builds released as "Coach WoW".
const LEGACY_IDENTIFIER: &str = "fr.coachwow.app";

/// Keeps settings, downloaded models and history from the "Coach WoW" builds.
fn migrate_data_dir(data: &std::path::Path) {
    if data.exists() {
        return;
    }
    let Some(legacy) = data.parent().map(|parent| parent.join(LEGACY_IDENTIFIER)) else {
        return;
    };
    if legacy.is_dir() {
        if let Err(error) = std::fs::rename(&legacy, data) {
            eprintln!("Could not move {} to {}: {error}", legacy.display(), data.display());
        }
    }
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn git_installed() -> bool {
    let candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ];
    if candidates.iter().any(|path| std::path::Path::new(path).is_file()) {
        return true;
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|directory| directory.join("git.exe").is_file()))
        .unwrap_or(false)
}

fn setup_status(coach: &Coach) -> Value {
    let settings = coach.settings();
    let (whisper, voice) = coach.models_ready();
    json!({
        "claude": claude::auth_status(&settings.claude_executable),
        "auth_mode": settings.auth_mode,
        "api_key_set": settings.api_key_protected.is_empty().then_some(false).unwrap_or_else(|| secret::unprotect(&settings.api_key_protected).is_ok()),
        "git": git_installed(),
        "models": { "whisper": whisper, "voice": voice },
        "engines": coach.engines(),
        "wow": wow::status(&settings.wow_folder, &coach.paths.addon),
    })
}

#[tauri::command]
async fn get_state(coach: CoachState<'_>) -> Result<Value, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (inputs, outputs) = audio::list_devices();
        json!({
            "settings": coach.settings(),
            "setup": setup_status(&coach),
            "phase": coach.phase(),
            "character": coach.character().or_else(|| coach.refresh_character(true)),
            "devices": { "inputs": inputs, "outputs": outputs },
            "choices": {
                "whisper": models::choices(&models::WHISPER_MODELS),
                "voices": tts_edge::VOICES.iter().map(|voice| json!({ "id": voice.id, "label": format!("{} · en ligne", voice.label), "size_mb": 0 }))
                    .chain(models::VOICES.iter().map(|voice| json!({ "id": voice.id, "label": format!("{} · hors ligne", voice.label), "size_mb": voice.size_mb })))
                    .collect::<Vec<_>>(),
            },
            "knowledge": coach.knowledge_overview(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn get_setup(coach: CoachState<'_>) -> Result<Value, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || setup_status(&coach)).await.map_err(err)
}

#[tauri::command]
async fn save_settings(app: tauri::AppHandle, coach: CoachState<'_>, settings: Settings) -> Result<(), String> {
    let coach = coach.inner().clone();
    let overlay_changed = {
        let old = coach.settings();
        old.overlay_enabled != settings.overlay_enabled || old.overlay_position != settings.overlay_position
    };
    let applied = settings.clone();
    tauri::async_runtime::spawn_blocking(move || coach.apply_settings(settings)).await.map_err(err)?.map_err(err)?;
    if overlay_changed {
        place_overlay(&app, &applied);
    }
    Ok(())
}

#[tauri::command]
fn install_claude() -> Result<(), String> {
    claude::install_claude().map_err(err)
}

#[tauri::command]
fn login_claude(coach: CoachState<'_>) -> Result<(), String> {
    claude::login_claude(&coach.settings().claude_executable).map_err(err)
}

#[tauri::command]
async fn logout_claude(coach: CoachState<'_>) -> Result<(), String> {
    let executable = coach.settings().claude_executable;
    tauri::async_runtime::spawn_blocking(move || claude::logout_claude(&executable)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
async fn set_api_key(coach: CoachState<'_>, key: String) -> Result<(), String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let key = key.trim().to_string();
        claude::check_api_key(&key).map_err(|error| format!("{error:#}"))?;
        let mut settings = coach.settings();
        settings.api_key_protected = secret::protect(&key).map_err(err)?;
        settings.auth_mode = "api_key".into();
        coach.apply_settings(settings).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn clear_api_key(coach: CoachState<'_>) -> Result<(), String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut settings = coach.settings();
        settings.api_key_protected.clear();
        settings.auth_mode = "subscription".into();
        coach.apply_settings(settings).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn get_knowledge(coach: CoachState<'_>) -> Value {
    coach.knowledge_overview()
}

/// Refreshes the knowledge base now and returns its new state with the report of the run.
#[tauri::command]
async fn sync_knowledge(coach: CoachState<'_>) -> Result<Value, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = coach.sync_knowledge();
        json!({ "overview": coach.knowledge_overview(), "report": report })
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    const ALLOWED: [&str; 5] = [
        "https://console.anthropic.com/",
        "https://claude.ai/",
        "https://news.blizzard.com/",
        "https://worldofwarcraft.blizzard.com/",
        "https://us.forums.blizzard.com/",
    ];
    if !ALLOWED.iter().any(|prefix| url.starts_with(prefix)) {
        return Err("adresse non autorisée".into());
    }
    let mut command = std::process::Command::new("rundll32");
    command.args(["url.dll,FileProtocolHandler", &url]);
    command.spawn().map(|_| ()).map_err(err)
}

#[tauri::command]
async fn enable_addon(coach: CoachState<'_>) -> Result<usize, String> {
    let folder = coach.settings().wow_folder;
    tauri::async_runtime::spawn_blocking(move || wow::enable_addon(&folder))
        .await
        .map_err(err)?
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn install_git() -> Result<(), String> {
    claude::open_console(
        "Write-Host 'Installation de Git pour Windows...' -ForegroundColor Cyan; \
         winget install --id Git.Git -e --accept-source-agreements --accept-package-agreements; \
         Write-Host 'Terminé.' -ForegroundColor Green; Read-Host 'Entrée pour fermer'",
    )
    .map_err(err)
}

#[tauri::command]
fn download_models(coach: CoachState<'_>) {
    coach.download_models();
}

#[tauri::command]
fn load_engines(coach: CoachState<'_>) {
    coach.load_engines();
}

#[tauri::command]
async fn capture_binding() -> Result<Option<String>, String> {
    let name = tauri::async_runtime::spawn_blocking(|| hotkeys::capture_next(Duration::from_secs(10))).await.map_err(err)?;
    Ok(match name.as_deref() {
        None | Some("key:esc") => None,
        Some("key:backspace") | Some("key:delete") => Some(String::new()),
        Some(other) => Some(other.to_string()),
    })
}

#[tauri::command]
async fn ask_text(coach: CoachState<'_>, text: String, screenshot: bool) -> Result<(), String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || coach.ask_text(text, screenshot)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
async fn take_screenshot(coach: CoachState<'_>) -> Result<Option<String>, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || coach.arm_screenshot()).await.map_err(err)
}

#[tauri::command]
fn clear_screenshot(coach: CoachState<'_>) {
    coach.clear_armed();
}

#[tauri::command]
fn test_beep(coach: CoachState<'_>) {
    coach.test_beep();
}

#[tauri::command]
fn stop(coach: CoachState<'_>) {
    coach.interrupt();
}

#[tauri::command]
fn new_session(coach: CoachState<'_>) {
    coach.new_session();
}

#[tauri::command]
async fn install_addon(coach: CoachState<'_>) -> Result<String, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || wow::install_addon(&coach.settings().wow_folder, &coach.paths.addon))
        .await
        .map_err(err)?
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn detect_wow() -> Option<String> {
    wow::detect_default_folder()
}

#[tauri::command]
async fn load_character(coach: CoachState<'_>) -> Result<Option<Value>, String> {
    let coach = coach.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = coach.settings().wow_folder;
        wow::load_snapshot(&folder).map_err(|error| format!("{error:#}"))?;
        Ok(coach.refresh_character(true).or_else(|| coach.character()))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn send_macro(coach: CoachState<'_>, name: String, body: String) -> Result<usize, String> {
    coach
        .send_macro(wow::Macro { name, body, icon: String::new(), per_character: false })
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn send_plan(coach: CoachState<'_>, steps: Vec<String>) -> Result<(), String> {
    coach.send_plan(steps).map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn open_data_folder(coach: CoachState<'_>) {
    let _ = std::process::Command::new("explorer").arg(&coach.paths.data).spawn();
}

const OVERLAY_WIDTH: f64 = 280.0;
const OVERLAY_HEIGHT: f64 = 64.0;

/// Puts the click-through indicator on the main screen, or hides it.
fn place_overlay(app: &tauri::AppHandle, settings: &Settings) {
    let Some(window) = app.get_webview_window("overlay") else {
        return;
    };
    let _ = window.set_ignore_cursor_events(true);
    if !settings.overlay_enabled {
        let _ = window.hide();
        return;
    }
    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let (left, top) = (monitor.position().x, monitor.position().y);
        let (width, height) = (monitor.size().width as i32, monitor.size().height as i32);
        let overlay_width = (OVERLAY_WIDTH * scale) as i32;
        let overlay_height = (OVERLAY_HEIGHT * scale) as i32;
        let margin = (14.0 * scale) as i32;
        let (x, y) = match settings.overlay_position.as_str() {
            "top-left" => (left + margin, top + margin),
            "top-right" => (left + width - overlay_width - margin, top + margin),
            "bottom" => (left + (width - overlay_width) / 2, top + height - overlay_height - margin * 6),
            _ => (left + (width - overlay_width) / 2, top + margin),
        };
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_always_on_top(true);
}

fn addon_resources(app: &tauri::App) -> PathBuf {
    let bundled = app.path().resource_dir().map(|dir| dir.join("addon").join(wow::ADDON_NAME)).unwrap_or_default();
    if bundled.join(format!("{}.toc", wow::ADDON_NAME)).is_file() {
        return bundled;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("addon").join(wow::ADDON_NAME)
}

fn init_logging(data: &std::path::Path) {
    let _ = std::fs::create_dir_all(data);
    if let Ok(file) = std::fs::File::create(data.join("coach.log")) {
        let config = simplelog::ConfigBuilder::new().add_filter_allow_str("mentor").build();
        let _ = simplelog::WriteLogger::init(log::LevelFilter::Debug, config, file);
    }
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(APP_IDENTIFIER)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--selftest") {
        let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)).unwrap_or_default();
        let mut addon = exe_dir.join("addon").join(wow::ADDON_NAME);
        if !addon.is_dir() {
            addon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("addon").join(wow::ADDON_NAME);
        }
        migrate_data_dir(&default_data_dir());
        let ok = selftest::run(&default_data_dir(), &addon, !args.iter().any(|arg| arg == "--no-claude"));
        std::process::exit(if ok { 0 } else { 1 });
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            migrate_data_dir(&data);
            init_logging(&data);
            let paths = Paths {
                models: data.join("models"),
                claude: data.join("claude"),
                settings: data.join("settings.json"),
                knowledge_state: data.join("knowledge.json"),
                addon: addon_resources(app),
                data,
            };
            log::info!("Mentor {} starting, data in {}", env!("CARGO_PKG_VERSION"), paths.data.display());
            let coach = Coach::new(app.handle().clone(), paths)?;
            let (sender, receiver) = crossbeam_channel::unbounded();
            hotkeys::start(sender);
            coach.start_input(receiver);
            let (whisper, voice) = coach.models_ready();
            if whisper && voice {
                coach.load_engines();
            }
            if coach.settings().always_on_top {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_always_on_top(true);
                }
            }
            place_overlay(app.handle(), &coach.settings());
            app.manage(coach);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_setup,
            save_settings,
            install_claude,
            login_claude,
            logout_claude,
            set_api_key,
            clear_api_key,
            open_url,
            get_knowledge,
            sync_knowledge,
            enable_addon,
            install_git,
            download_models,
            load_engines,
            capture_binding,
            ask_text,
            take_screenshot,
            clear_screenshot,
            test_beep,
            stop,
            new_session,
            install_addon,
            detect_wow,
            load_character,
            send_macro,
            send_plan,
            open_data_folder,
        ])
        .on_window_event(|window, event| {
            // Closing the main window quits the app (the overlay must not keep it alive).
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    if let Some(coach) = window.app_handle().try_state::<Arc<Coach>>() {
                        coach.shutdown();
                    }
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Mentor");
}
