//! The coach: push-to-talk state machine, question pipeline, speech and game integration.

use crate::audio::{Player, Recorder, CAPTURE_RATE};
use crate::claude::{self, ClaudeBackend, ClaudeOptions};
use crate::hotkeys::InputEvent;
use crate::knowledge::{self, Knowledge};
use crate::models;
use crate::screen::{self, Shot};
use crate::settings::Settings;
use crate::stt::Transcriber;
use crate::text::{KeywordMatcher, SentenceSplitter, SpeechFilter};
use crate::tts::Speaker;
use crate::wow::{self, Macro};
use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

pub const PROMPT_TEMPLATE: &str = include_str!("../resources/coach_prompt.md");
/// What the coach must know about World of Warcraft: Forever before anything else.
const FOREVER_NOTES: &str = include_str!("../resources/forever_notes.md");
const OTHER_GAME_NOTES: &str = "The game\n- Your built-in knowledge of this game may be outdated or wrong: the game or its latest patch may be newer than your training data. Never answer a game-specific question from memory alone.";

fn is_forever(settings: &Settings) -> bool {
    settings.game.to_lowercase().contains("forever")
}

#[derive(Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub models: PathBuf,
    pub claude: PathBuf,
    pub settings: PathBuf,
    pub addon: PathBuf,
    /// Bookkeeping of the knowledge base (kept outside the folder Claude can read).
    pub knowledge_state: PathBuf,
}

const WORK_IDLE: u8 = 0;
const WORK_TRANSCRIBING: u8 = 1;
const WORK_THINKING: u8 = 2;

enum JobInput {
    Audio(Vec<f32>),
    Text(String),
}

struct Job {
    id: u64,
    input: JobInput,
    shot: Option<Shot>,
    forced: bool,
    cancel: Arc<AtomicBool>,
    generation: u64,
    started: Instant,
}

struct SpeechItem {
    cancel: Arc<AtomicBool>,
    generation: u64,
    text: String,
    first_audio: Option<Arc<Mutex<Option<Instant>>>>,
}

/// Sentences waiting to be played while their voice is being prepared. With the one the
/// player thread is waiting for and the one about to enter the queue, three are prepared
/// at the same time: the voice stays ahead of the playback even when the service is slow.
const SPEECH_QUEUE: usize = 1;

type Audio = (Vec<f32>, u32);

struct Recording {
    action: &'static str,
    id: u64,
    shot: Option<Receiver<Option<Shot>>>,
    forced: bool,
}

struct CharacterState {
    snapshot: wow::Snapshot,
    /// Conversation (see `ClaudeBackend::generation`) that already received this sheet.
    sent_generation: Option<u64>,
}

#[derive(Serialize, Clone, Default)]
pub struct EngineStatus {
    pub ready: bool,
    pub loading: bool,
    pub stt: Option<String>,
    pub tts: bool,
    pub error: Option<String>,
}

pub struct Coach {
    app: AppHandle,
    pub paths: Paths,
    settings: RwLock<Settings>,
    backend: ClaudeBackend,
    stt: RwLock<Option<Arc<Transcriber>>>,
    tts: RwLock<Option<Arc<Speaker>>>,
    player: RwLock<Option<Arc<Player>>>,
    recorder: Mutex<Option<Recorder>>,
    keywords: RwLock<KeywordMatcher>,
    speech_tx: Sender<SpeechItem>,
    job_tx: Sender<Job>,
    recording: Mutex<Option<Recording>>,
    active: Mutex<Vec<Arc<AtomicBool>>>,
    ids: AtomicU64,
    armed: Mutex<Option<(Shot, Instant)>>,
    work: AtomicU8,
    engines: Mutex<EngineStatus>,
    character: Mutex<Option<CharacterState>>,
    character_stamp: Mutex<Option<SystemTime>>,
    pending_macros: Mutex<Vec<Macro>>,
    inbox_written_at: AtomicU64,
    knowledge: Knowledge,
    /// Prompt and options last given to Claude, to restart it only when they change.
    applied: Mutex<(String, ClaudeOptions)>,
}

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or_default()
}

pub fn claude_options(settings: &Settings) -> ClaudeOptions {
    ClaudeOptions {
        executable: settings.claude_executable.clone(),
        model: settings.claude_model.clone(),
        effort: settings.claude_effort.clone(),
        api_key: settings.api_key(),
    }
}

/// `knowledge` is the description of the local knowledge base, see `Knowledge::prompt_section`.
pub fn prompt_for(settings: &Settings, knowledge: &str) -> String {
    let notes = if is_forever(settings) { FOREVER_NOTES } else { OTHER_GAME_NOTES };
    claude::render_prompt(PROMPT_TEMPLATE, &settings.game, &settings.language, &settings.preferred_sources, notes, knowledge)
}

/// The knowledge base only covers World of Warcraft: Forever.
pub fn knowledge_active(settings: &Settings) -> bool {
    settings.knowledge_enabled && is_forever(settings)
}

pub fn knowledge_section(settings: &Settings, knowledge: &Knowledge) -> String {
    if knowledge_active(settings) {
        knowledge.prompt_section()
    } else {
        String::new()
    }
}

impl Coach {
    pub fn new(app: AppHandle, paths: Paths) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&paths.data)?;
        let mut settings = Settings::load(&paths.settings);
        if settings.wow_folder.is_empty() {
            if let Some(folder) = wow::detect_default_folder() {
                settings.wow_folder = folder;
                let _ = settings.save(&paths.settings);
            }
        }
        let knowledge = Knowledge::open(&paths.claude, paths.knowledge_state.clone());
        let prompt = prompt_for(&settings, &knowledge_section(&settings, &knowledge));
        let options = claude_options(&settings);
        let backend = ClaudeBackend::new(paths.claude.clone(), &prompt, options.clone())?;
        let (speech_tx, speech_rx) = unbounded();
        let (job_tx, job_rx) = unbounded();
        let coach = Arc::new(Self {
            app,
            keywords: RwLock::new(KeywordMatcher::new(&settings.screenshot_keywords)),
            settings: RwLock::new(settings),
            paths,
            backend,
            stt: RwLock::new(None),
            tts: RwLock::new(None),
            player: RwLock::new(None),
            recorder: Mutex::new(None),
            speech_tx,
            job_tx,
            recording: Mutex::new(None),
            active: Mutex::new(Vec::new()),
            ids: AtomicU64::new(1),
            armed: Mutex::new(None),
            work: AtomicU8::new(WORK_IDLE),
            engines: Mutex::new(EngineStatus::default()),
            character: Mutex::new(None),
            character_stamp: Mutex::new(None),
            pending_macros: Mutex::new(Vec::new()),
            inbox_written_at: AtomicU64::new(0),
            knowledge,
            applied: Mutex::new((prompt, options)),
        });
        coach.spawn_workers(speech_rx, job_rx);
        Ok(coach)
    }

    fn spawn_workers(self: &Arc<Self>, speech_rx: Receiver<SpeechItem>, job_rx: Receiver<Job>) {
        let (prepared_tx, prepared_rx) = crossbeam_channel::bounded::<(SpeechItem, Receiver<Option<Audio>>)>(SPEECH_QUEUE);
        let coach = self.clone();
        std::thread::Builder::new()
            .name("speech".into())
            .spawn(move || {
                for item in speech_rx {
                    if item.cancel.load(Ordering::SeqCst) {
                        continue;
                    }
                    let (audio_tx, audio_rx) = crossbeam_channel::bounded::<Option<Audio>>(1);
                    let worker = coach.clone();
                    let (text, cancel) = (item.text.clone(), item.cancel.clone());
                    std::thread::spawn(move || {
                        let _ = audio_tx.send(worker.synthesize(&text, &cancel));
                    });
                    if prepared_tx.send((item, audio_rx)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        let coach = self.clone();
        std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || {
                // Sentences are played in the order they were written, whatever the order
                // their audio became ready in.
                for (item, audio) in prepared_rx {
                    if let Ok(Some(audio)) = audio.recv() {
                        coach.play_item(item, audio);
                    }
                }
            })
            .ok();
        let coach = self.clone();
        std::thread::Builder::new()
            .name("jobs".into())
            .spawn(move || {
                for job in job_rx {
                    let cancel = job.cancel.clone();
                    if !cancel.load(Ordering::SeqCst) {
                        coach.handle(job);
                    }
                    coach.work.store(WORK_IDLE, Ordering::SeqCst);
                    coach.active.lock().retain(|flag| !Arc::ptr_eq(flag, &cancel));
                }
            })
            .ok();
        let coach = self.clone();
        std::thread::Builder::new()
            .name("status".into())
            .spawn(move || {
                let mut last = String::new();
                let mut last_character_check = Instant::now() - Duration::from_secs(10);
                loop {
                    let phase = coach.phase();
                    if phase != last {
                        let _ = coach.app.emit("status", json!({ "phase": phase }));
                        last = phase.to_string();
                    }
                    if last_character_check.elapsed() > Duration::from_secs(3) {
                        last_character_check = Instant::now();
                        coach.refresh_character(false);
                    }
                    if phase == "listening" {
                        let level = coach.recorder.lock().as_ref().map(Recorder::level).unwrap_or_default();
                        let _ = coach.app.emit("mic_level", json!({ "level": level }));
                        std::thread::sleep(Duration::from_millis(50));
                    } else {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
        let coach = self.clone();
        std::thread::Builder::new()
            .name("knowledge".into())
            .spawn(move || {
                let mut next_attempt = Instant::now();
                loop {
                    let due = now_epoch().saturating_sub(coach.knowledge.last_sync()) >= knowledge::SYNC_INTERVAL;
                    if due && Instant::now() >= next_attempt && knowledge_active(&coach.settings()) {
                        coach.sync_knowledge();
                        // Offline or site down: try again later instead of hammering.
                        next_attempt = Instant::now() + Duration::from_secs(600);
                    }
                    std::thread::sleep(Duration::from_secs(20));
                }
            })
            .ok();
    }

    pub fn start_input(self: &Arc<Self>, events: Receiver<InputEvent>) {
        let coach = self.clone();
        std::thread::Builder::new()
            .name("input".into())
            .spawn(move || {
                for event in events {
                    let actions = {
                        let settings = coach.settings.read();
                        let mut actions: Vec<&'static str> = Vec::new();
                        for name in &event.names {
                            for action in settings.bindings.actions_for(name) {
                                if !actions.contains(&action) {
                                    actions.push(action);
                                }
                            }
                        }
                        actions
                    };
                    for action in actions {
                        if event.pressed {
                            coach.on_press(action);
                        } else {
                            coach.on_release(action);
                        }
                    }
                }
            })
            .ok();
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn engines(&self) -> EngineStatus {
        self.engines.lock().clone()
    }

    pub fn phase(&self) -> &'static str {
        let engines = self.engines.lock().clone();
        if engines.loading {
            return "loading";
        }
        if !engines.ready {
            return "setup";
        }
        if self.recording.lock().is_some() {
            return "listening";
        }
        match self.work.load(Ordering::SeqCst) {
            WORK_TRANSCRIBING => return "transcribing",
            WORK_THINKING => return "thinking",
            _ => {}
        }
        if self.player.read().as_ref().is_some_and(|player| player.is_busy()) {
            return "speaking";
        }
        "ready"
    }

    fn notice(&self, level: &str, text: &str) {
        let _ = self.app.emit("notice", json!({ "level": level, "text": text }));
    }

    fn system_message(&self, text: &str) {
        self.system_message_with(text, json!([]));
    }

    /// `actions` are buttons under the message: `[{ "label": "...", "ask": "question sent when clicked" }]`.
    fn system_message_with(&self, text: &str, actions: Value) {
        let id = format!("s{}", self.ids.fetch_add(1, Ordering::SeqCst));
        let _ = self.app.emit("chat", json!({ "id": id, "role": "system", "text": text, "actions": actions }));
    }

    fn tone(&self, name: &str, generation: Option<u64>) {
        let (enabled, volume) = {
            let settings = self.settings.read();
            (settings.beeps, settings.beep_volume)
        };
        if !enabled {
            return;
        }
        if let Some(player) = self.player.read().as_ref() {
            player.play_tone(name, generation, volume);
        }
    }

    pub fn test_beep(&self) {
        let volume = self.settings.read().beep_volume;
        if let Some(player) = self.player.read().as_ref() {
            player.play_tone("start", None, volume);
        }
    }

    fn say_system(&self, text: &str) {
        if !self.settings.read().speak_answers {
            return;
        }
        let generation = self.player.read().as_ref().map(|player| player.generation()).unwrap_or_default();
        let _ = self.speech_tx.send(SpeechItem { cancel: Arc::new(AtomicBool::new(false)), generation, text: text.to_string(), first_audio: None });
    }

    /// Audio of one sentence, or nothing when it was cancelled or the voice failed.
    fn synthesize(&self, text: &str, cancel: &AtomicBool) -> Option<Audio> {
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        let tts = self.tts.read().clone()?;
        tts.synthesize(text).map_err(|error| log::error!("Speech synthesis failed: {error}")).ok()
    }

    fn play_item(&self, item: SpeechItem, (samples, rate): Audio) {
        if item.cancel.load(Ordering::SeqCst) || samples.is_empty() {
            return;
        }
        if let Some(player) = self.player.read().as_ref() {
            player.play(&samples, rate, item.generation);
        }
        if let Some(first) = item.first_audio {
            first.lock().get_or_insert_with(Instant::now);
        }
    }

    // ---------------------------------------------------------------- engines

    pub fn models_ready(&self) -> (bool, bool) {
        let settings = self.settings.read();
        (models::whisper_ready(&self.paths.models, &settings.whisper_model), models::voice_ready(&self.paths.models, &settings.voice))
    }

    pub fn load_engines(self: &Arc<Self>) {
        let coach = self.clone();
        std::thread::spawn(move || coach.load_engines_blocking());
    }

    fn load_engines_blocking(&self) {
        {
            let mut engines = self.engines.lock();
            if engines.loading {
                return;
            }
            *engines = EngineStatus { loading: true, ..Default::default() };
        }
        let _ = self.app.emit("engines", self.engines());
        let result = self.load_engines_inner();
        {
            let mut engines = self.engines.lock();
            engines.loading = false;
            match &result {
                Ok(description) => {
                    engines.ready = true;
                    engines.stt = Some(description.clone());
                    engines.tts = self.tts.read().is_some();
                    engines.error = None;
                }
                Err(error) => {
                    engines.ready = false;
                    engines.error = Some(error.to_string());
                }
            }
        }
        let _ = self.app.emit("engines", self.engines());
        match result {
            Ok(_) => {
                self.backend.warm_up();
                self.say_system(if self.settings.read().language == "fr" { "Mentor est prêt." } else { "Mentor is ready." });
            }
            Err(error) => log::error!("Engines failed to load: {error:#}"),
        }
    }

    fn load_engines_inner(&self) -> Result<String> {
        let settings = self.settings();
        let (whisper_ok, voice_ok) = self.models_ready();
        if !whisper_ok || !voice_ok {
            return Err(anyhow!("modèles à télécharger"));
        }
        if self.player.read().is_none() {
            *self.player.write() = Some(Arc::new(Player::new(&settings.output_device, settings.volume)?));
        }
        if self.recorder.lock().is_none() {
            *self.recorder.lock() = Some(Recorder::open(&settings.input_device)?);
        }
        self.load_stt(&settings)?;
        self.load_tts(&settings)?;
        Ok(self.stt.read().as_ref().map(|stt| stt.description.clone()).unwrap_or_default())
    }

    fn load_stt(&self, settings: &Settings) -> Result<()> {
        let path = models::whisper_path(&self.paths.models, &settings.whisper_model);
        *self.stt.write() = None;
        let transcriber = match Transcriber::load(&path, settings.use_gpu, &settings.language, &settings.initial_prompt) {
            Ok(transcriber) => transcriber,
            Err(error) if settings.use_gpu => {
                log::warn!("Whisper on GPU failed ({error}), using the CPU");
                Transcriber::load(&path, false, &settings.language, &settings.initial_prompt)?
            }
            Err(error) => return Err(error),
        };
        log::info!("Speech recognition: {}", transcriber.description);
        *self.stt.write() = Some(Arc::new(transcriber));
        Ok(())
    }

    fn load_tts(&self, settings: &Settings) -> Result<()> {
        let speaker = if crate::tts_edge::is_edge_voice(&settings.voice) {
            let piper = models::voice_dir(&self.paths.models, models::VOICES[0].id);
            Speaker::with_edge(&settings.voice, piper.is_dir().then_some(piper.as_path()), settings.voice_speed)?
        } else {
            Speaker::load(&models::voice_dir(&self.paths.models, &settings.voice), settings.voice_speed)?
        };
        *self.tts.write() = Some(Arc::new(speaker));
        Ok(())
    }

    pub fn download_models(self: &Arc<Self>) {
        let coach = self.clone();
        std::thread::spawn(move || {
            let settings = coach.settings();
            let emit = |kind: &str, state: &str, done: u64, total: u64, error: Option<String>| {
                let _ = coach.app.emit("download", json!({ "kind": kind, "state": state, "done": done, "total": total, "error": error }));
            };
            let mut failed = false;
            if !models::whisper_ready(&coach.paths.models, &settings.whisper_model) {
                let result = models::download_whisper(&coach.paths.models, &settings.whisper_model, &mut |done, total| emit("whisper", "progress", done, total, None));
                match result {
                    Ok(()) => emit("whisper", "done", 1, 1, None),
                    Err(error) => {
                        failed = true;
                        emit("whisper", "error", 0, 0, Some(format!("{error:#}")));
                    }
                }
            }
            if !models::voice_ready(&coach.paths.models, &settings.voice) {
                let result = models::download_voice(&coach.paths.models, &settings.voice, &mut |done, total| emit("voice", "progress", done, total, None));
                match result {
                    Ok(()) => emit("voice", "done", 1, 1, None),
                    Err(error) => {
                        failed = true;
                        emit("voice", "error", 0, 0, Some(format!("{error:#}")));
                    }
                }
            }
            if !failed {
                coach.load_engines_blocking();
            }
        });
    }

    pub fn apply_settings(self: &Arc<Self>, new: Settings) -> Result<()> {
        let old = self.settings();
        new.save(&self.paths.settings)?;
        *self.settings.write() = new.clone();
        *self.keywords.write() = KeywordMatcher::new(&new.screenshot_keywords);
        self.reconfigure(&new);
        if let Some(player) = self.player.read().as_ref() {
            player.set_volume(new.volume);
            if old.output_device != new.output_device {
                player.open(&new.output_device)?;
            }
        }
        if old.input_device != new.input_device && self.recorder.lock().is_some() {
            *self.recorder.lock() = Some(Recorder::open(&new.input_device)?);
        }
        let engines_ready = self.engines.lock().ready;
        let stt_changed = (old.whisper_model.as_str(), old.use_gpu, old.language.as_str(), old.initial_prompt.as_str())
            != (new.whisper_model.as_str(), new.use_gpu, new.language.as_str(), new.initial_prompt.as_str());
        let tts_changed = old.voice != new.voice || (old.voice_speed - new.voice_speed).abs() > f32::EPSILON;
        if stt_changed || tts_changed {
            let (whisper_ok, voice_ok) = self.models_ready();
            if !whisper_ok || !voice_ok {
                self.engines.lock().ready = false;
                let _ = self.app.emit("engines", self.engines());
            } else if engines_ready || stt_changed || tts_changed {
                let coach = self.clone();
                std::thread::spawn(move || {
                    let result = (|| -> Result<()> {
                        if stt_changed {
                            coach.load_stt(&new)?;
                        }
                        if tts_changed {
                            coach.load_tts(&new)?;
                        }
                        Ok(())
                    })();
                    if let Err(error) = result {
                        coach.notice("error", &format!("Rechargement impossible : {error:#}"));
                        coach.engines.lock().error = Some(error.to_string());
                    } else if !coach.engines.lock().ready {
                        coach.load_engines_blocking();
                    }
                    let _ = coach.app.emit("engines", coach.engines());
                });
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- input

    fn engines_ready(&self) -> bool {
        self.engines.lock().ready && self.stt.read().is_some()
    }

    pub fn on_press(self: &Arc<Self>, action: &'static str) {
        match action {
            "push_to_talk" | "push_to_talk_with_screenshot" => self.begin_recording(action),
            "screenshot" => {
                let coach = self.clone();
                std::thread::spawn(move || coach.arm_screenshot());
            }
            "stop" => {
                self.interrupt();
                self.tone("stop", None);
            }
            "new_session" => self.new_session(),
            _ => {}
        }
    }

    pub fn on_release(self: &Arc<Self>, action: &'static str) {
        if self.recording.lock().as_ref().is_none_or(|recording| recording.action != action) {
            return;
        }
        let tail = Duration::from_millis(self.settings.read().release_tail_ms);
        let coach = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(tail);
            coach.finish_recording();
        });
    }

    fn begin_recording(self: &Arc<Self>, action: &'static str) {
        if !self.engines_ready() {
            self.tone("error", None);
            self.notice("warning", "Le coach n'est pas encore prêt : termine la configuration.");
            return;
        }
        let settings = self.settings();
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        {
            let mut recording = self.recording.lock();
            if recording.is_some() {
                return;
            }
            *recording = Some(Recording { action, id, shot: None, forced: false });
        }
        if settings.interrupt_on_push_to_talk {
            self.interrupt();
        }
        self.tone("start", None);
        if let Some(recorder) = self.recorder.lock().as_ref() {
            recorder.start();
        }
        let forced = action == "push_to_talk_with_screenshot";
        if forced || settings.screenshot_auto {
            let (sender, receiver) = crossbeam_channel::bounded(1);
            let max_width = settings.screenshot_max_width;
            std::thread::spawn(move || {
                let shot = screen::capture(max_width).map_err(|error| log::warn!("Screenshot failed: {error}")).ok();
                let _ = sender.send(shot);
            });
            if let Some(recording) = self.recording.lock().as_mut() {
                recording.shot = Some(receiver);
                recording.forced = forced;
            }
        }
    }

    fn finish_recording(self: &Arc<Self>) {
        let audio = self.recorder.lock().as_ref().map(Recorder::stop).unwrap_or_default();
        let Some(recording) = self.recording.lock().take() else {
            return;
        };
        let mut shot = recording.shot.and_then(|receiver| receiver.recv_timeout(Duration::from_secs(3)).ok().flatten());
        let mut forced = recording.forced;
        let settings = self.settings();
        if (audio.len() as f32) < CAPTURE_RATE as f32 * settings.min_recording_seconds {
            self.tone("error", None);
            return;
        }
        if let Some(armed) = self.take_armed() {
            shot = Some(armed);
            forced = true;
        }
        self.enqueue(recording.id, JobInput::Audio(audio), shot, forced);
    }

    fn enqueue(&self, id: u64, input: JobInput, shot: Option<Shot>, forced: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.active.lock().push(cancel.clone());
        let generation = self.player.read().as_ref().map(|player| player.generation()).unwrap_or_default();
        if matches!(input, JobInput::Audio(_)) {
            self.tone("stop", Some(generation));
        }
        let _ = self.job_tx.send(Job { id, input, shot, forced, cancel, generation, started: Instant::now() });
    }

    pub fn ask_text(self: &Arc<Self>, text: String, with_screenshot: bool) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let settings = self.settings();
        if settings.interrupt_on_push_to_talk {
            self.interrupt();
        }
        let mut shot = None;
        let mut forced = false;
        if let Some(armed) = self.take_armed() {
            shot = Some(armed);
            forced = true;
        } else if with_screenshot {
            shot = Some(screen::capture(settings.screenshot_max_width)?);
            forced = true;
        }
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        self.enqueue(id, JobInput::Text(text), shot, forced);
        Ok(())
    }

    fn take_armed(&self) -> Option<Shot> {
        let (shot, taken) = self.armed.lock().take()?;
        (taken.elapsed() < Duration::from_secs(self.settings.read().screenshot_armed_seconds)).then_some(shot)
    }

    pub fn arm_screenshot(&self) -> Option<String> {
        match screen::capture(self.settings.read().screenshot_max_width) {
            Ok(shot) => {
                let thumbnail = shot.thumbnail.clone();
                if screen::is_mostly_black(&shot) {
                    self.notice("warning", "La capture est noire : passe le jeu en « plein écran fenêtré ».");
                }
                *self.armed.lock() = Some((shot, Instant::now()));
                self.tone("reset", None);
                self.say_system(if self.settings.read().language == "fr" { "Capture prise." } else { "Screenshot taken." });
                let _ = self.app.emit("armed", json!({ "thumbnail": thumbnail }));
                Some(thumbnail)
            }
            Err(error) => {
                self.tone("error", None);
                self.notice("error", &format!("Capture impossible : {error}"));
                None
            }
        }
    }

    pub fn clear_armed(&self) {
        self.armed.lock().take();
    }

    pub fn interrupt(&self) {
        for flag in self.active.lock().iter() {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(player) = self.player.read().as_ref() {
            player.stop();
        }
    }

    pub fn new_session(self: &Arc<Self>) {
        self.interrupt();
        self.tone("reset", None);
        self.say_system(if self.settings.read().language == "fr" { "Nouvelle session." } else { "New session." });
        self.system_message("Nouvelle session : le coach a oublié la conversation.");
        let coach = self.clone();
        std::thread::spawn(move || coach.backend.reset());
    }

    // ---------------------------------------------------------------- questions

    fn handle(&self, job: Job) {
        let settings = self.settings();
        let question = match job.input {
            JobInput::Audio(audio) => {
                self.work.store(WORK_TRANSCRIBING, Ordering::SeqCst);
                let Some(stt) = self.stt.read().clone() else {
                    return;
                };
                match stt.transcribe(&audio) {
                    Ok(text) => text,
                    Err(error) => {
                        self.notice("error", &format!("Reconnaissance vocale : {error}"));
                        String::new()
                    }
                }
            }
            JobInput::Text(text) => text,
        };
        let transcribed = job.started.elapsed();
        if question.is_empty() {
            self.tone("error", Some(job.generation));
            self.notice("warning", "Je n'ai rien compris, réessaie.");
            return;
        }
        if job.cancel.load(Ordering::SeqCst) {
            return;
        }
        let shot = match job.shot {
            Some(shot) if job.forced || self.keywords.read().matches(&question) => Some(shot),
            _ => None,
        };
        let user_id = format!("u{}", job.id);
        let coach_id = format!("c{}", job.id);
        let _ = self.app.emit(
            "chat",
            json!({ "id": user_id, "role": "user", "text": question, "image": shot.as_ref().map(|shot| shot.thumbnail.clone()), "auto_shot": shot.is_some() && !job.forced }),
        );
        self.work.store(WORK_THINKING, Ordering::SeqCst);
        let _ = self.app.emit("chat", json!({ "id": coach_id, "role": "coach", "text": "", "pending": true }));
        let generation = self.backend.begin_turn();
        let (message_text, sheet_sent) = self.compose_question(&question, &settings, generation);
        let message = ClaudeBackend::build_message(&message_text, shot.as_ref().map(|shot| shot.jpeg.as_slice()));
        let mut filter = SpeechFilter::default();
        let mut splitter = SentenceSplitter::default();
        let first_audio = Arc::new(Mutex::new(None::<Instant>));
        let speak = settings.speak_answers;
        let mut on_text = |chunk: &str| {
            if job.cancel.load(Ordering::SeqCst) {
                return;
            }
            let _ = self.app.emit("chat_delta", json!({ "id": coach_id, "text": chunk }));
            if speak {
                for sentence in splitter.feed(&filter.push(chunk)) {
                    let _ = self.speech_tx.send(SpeechItem {
                        cancel: job.cancel.clone(),
                        generation: job.generation,
                        text: sentence,
                        first_audio: Some(first_audio.clone()),
                    });
                }
            }
        };
        let result = self.backend.ask(&message, &job.cancel, &mut on_text);
        if job.cancel.load(Ordering::SeqCst) {
            let _ = self.app.emit("chat_done", json!({ "id": coach_id, "interrupted": true }));
            return;
        }
        match result {
            Ok(answer) => {
                if speak {
                    let rest = filter.finish();
                    let mut sentences = splitter.feed(&rest);
                    sentences.extend(splitter.flush());
                    for sentence in sentences {
                        let _ = self.speech_tx.send(SpeechItem {
                            cancel: job.cancel.clone(),
                            generation: job.generation,
                            text: sentence,
                            first_audio: Some(first_audio.clone()),
                        });
                    }
                }
                // Only when this very sheet went to the conversation that is still current.
                if let Some(sheet) = sheet_sent.filter(|_| self.backend.generation() == generation) {
                    if let Some(character) = self.character.lock().as_mut().filter(|character| character.snapshot.updated_at == sheet) {
                        character.sent_generation = Some(generation);
                    }
                }
                let total = job.started.elapsed();
                let first = (*first_audio.lock()).map(|instant| instant.duration_since(job.started));
                let _ = self.app.emit(
                    "chat_done",
                    json!({
                        "id": coach_id,
                        "text": answer,
                        "timing": { "transcription": transcribed.as_secs_f32(), "first_audio": first.map(|d| d.as_secs_f32()), "total": total.as_secs_f32() }
                    }),
                );
            }
            Err(error) => {
                let detail = format!("{error:#}");
                log::error!("Backend error: {detail}");
                self.tone("error", Some(job.generation));
                let lower = detail.to_lowercase();
                let hint = if settings.uses_api_key() && (lower.contains("credit") || lower.contains("billing")) {
                    " Ton compte API n'a plus de crédit : recharge-le sur console.anthropic.com."
                } else if settings.uses_api_key() && (lower.contains("api key") || lower.contains("401")) {
                    " Ta clé API est refusée : remplace-la dans les réglages."
                } else if lower.contains("login") || lower.contains("auth") {
                    " Reconnecte-toi à Claude dans les réglages."
                } else {
                    ""
                };
                let _ = self.app.emit("chat_done", json!({ "id": coach_id, "error": format!("{detail}{hint}") }));
                self.say_system(if settings.language == "fr" { "Je n'ai pas pu obtenir de réponse." } else { "I could not get an answer." });
            }
        }
    }

    /// Adds the character sheet to the question when the conversation `generation` has not
    /// received it yet. Returns the export time of the sheet that was added.
    fn compose_question(&self, question: &str, settings: &Settings, generation: u64) -> (String, Option<i64>) {
        if !settings.share_character {
            return (question.to_string(), None);
        }
        let character = self.character.lock();
        match character.as_ref() {
            Some(state) if state.sent_generation != Some(generation) => (
                format!("{}\n\n---\nMy question: {question}", wow::format_snapshot(&state.snapshot)),
                Some(state.snapshot.updated_at),
            ),
            _ => (question.to_string(), None),
        }
    }

    // ---------------------------------------------------------------- knowledge base

    /// Gives Claude the current prompt (settings and knowledge base) and options, when they changed.
    fn reconfigure(self: &Arc<Self>, settings: &Settings) {
        let prompt = prompt_for(settings, &knowledge_section(settings, &self.knowledge));
        let options = claude_options(settings);
        {
            let mut applied = self.applied.lock();
            if applied.0 == prompt && applied.1 == options {
                return;
            }
            *applied = (prompt.clone(), options.clone());
        }
        self.backend.configure(&prompt, options);
        if self.engines.lock().ready {
            let coach = self.clone();
            std::thread::spawn(move || coach.backend.warm_up());
        }
    }

    pub fn knowledge_overview(&self) -> Value {
        let settings = self.settings.read();
        let mut overview = self.knowledge.overview(&settings.language);
        overview["enabled"] = json!(knowledge_active(&settings));
        overview
    }

    /// Fetches what changed on the official sites, then tells Claude and the window.
    /// Returns `None` when a synchronization was already running.
    pub fn sync_knowledge(self: &Arc<Self>) -> Option<knowledge::SyncReport> {
        let language = self.settings.read().language.clone();
        let _ = self.app.emit("knowledge_syncing", json!({}));
        let report = self.knowledge.sync(&language)?;
        self.reconfigure(&self.settings());
        let _ = self.app.emit("knowledge", self.knowledge_overview());
        self.announce_knowledge(&report, &language);
        Some(report)
    }

    fn announce_knowledge(&self, report: &knowledge::SyncReport, language: &str) {
        if report.changes.is_empty() {
            return;
        }
        if report.first || report.changes.len() > 12 {
            self.system_message(&format!(
                "Base de connaissances {} : {} documents officiels de Blizzard (correctifs, annonces des développeurs, guides). Elle se met à jour toute seule.",
                if report.first { "prête" } else { "mise à jour" },
                report.total
            ));
            return;
        }
        let titles: Vec<String> = report
            .changes
            .iter()
            .take(3)
            .map(|change| format!("« {} »{}", change.title, if change.new { "" } else { " (mis à jour)" }))
            .collect();
        let more = match report.changes.len().saturating_sub(3) {
            0 => String::new(),
            1 => " et un autre document".to_string(),
            count => format!(" et {count} autres documents"),
        };
        let (label, ask) = if language == "fr" {
            (
                "Qu'est-ce qui change pour moi ?",
                "Qu'est-ce qui change pour mon personnage dans les dernières notes officielles de Blizzard (correctifs et annonces des développeurs) ? Résume en quelques phrases.",
            )
        } else {
            (
                "What changes for me?",
                "What changes for my character in the latest official Blizzard notes (hotfixes and developer posts)? Summarize in a few sentences.",
            )
        };
        self.system_message_with(&format!("Nouveau chez Blizzard : {}{more}.", titles.join(", ")), json!([{ "label": label, "ask": ask }]));
    }

    // ---------------------------------------------------------------- game integration

    /// Reloads the character sheet when the SavedVariables file changed (or always when forced).
    pub fn refresh_character(&self, force: bool) -> Option<Value> {
        let folder = self.settings.read().wow_folder.clone();
        if folder.is_empty() {
            return None;
        }
        let modified = wow::saved_variables_modified(&folder);
        {
            let mut stamp = self.character_stamp.lock();
            if !force && modified.is_some() && *stamp == modified {
                return None;
            }
            if modified.is_none() {
                return None;
            }
            *stamp = modified;
        }
        match wow::load_snapshot(&folder) {
            Ok(snapshot) => {
                let summary = json!({ "key": snapshot.key, "data": snapshot.data });
                if snapshot.updated_at as u64 > self.inbox_written_at.load(Ordering::SeqCst) {
                    self.pending_macros.lock().clear();
                }
                let mut character = self.character.lock();
                let changed = character.as_ref().is_none_or(|state| state.snapshot.updated_at != snapshot.updated_at || state.snapshot.key != snapshot.key);
                if changed {
                    *character = Some(CharacterState { snapshot, sent_generation: None });
                    drop(character);
                    let _ = self.app.emit("character", &summary);
                }
                Some(summary)
            }
            Err(error) => {
                log::warn!("Character sheet: {error:#}");
                None
            }
        }
    }

    pub fn character(&self) -> Option<Value> {
        self.character.lock().as_ref().map(|state| json!({ "key": state.snapshot.key, "data": state.snapshot.data }))
    }

    fn character_key(&self) -> String {
        self.character.lock().as_ref().map(|state| state.snapshot.key.clone()).unwrap_or_default()
    }

    pub fn send_macro(&self, item: Macro) -> Result<usize> {
        let item = wow::validate_macro(&item)?;
        let folder = self.settings.read().wow_folder.clone();
        let macros = {
            let mut pending = self.pending_macros.lock();
            pending.retain(|existing| existing.name != item.name);
            pending.push(item);
            pending.clone()
        };
        wow::write_inbox(&folder, &self.character_key(), &macros, &[], &[])?;
        self.inbox_written_at.store(now_epoch(), Ordering::SeqCst);
        Ok(macros.len())
    }

    pub fn send_plan(&self, steps: Vec<String>) -> Result<()> {
        let folder = self.settings.read().wow_folder.clone();
        let macros = self.pending_macros.lock().clone();
        let steps: Vec<String> = steps.into_iter().map(|step| step.trim().to_string()).filter(|step| !step.is_empty()).collect();
        if steps.is_empty() {
            return Err(anyhow!("le plan est vide"));
        }
        wow::write_inbox(&folder, &self.character_key(), &macros, &steps, &[])?;
        self.inbox_written_at.store(now_epoch(), Ordering::SeqCst);
        Ok(())
    }

    pub fn shutdown(&self) {
        self.interrupt();
        self.backend.close();
    }
}
