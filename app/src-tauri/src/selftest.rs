//! `mentor.exe --selftest`: checks every component without the window and writes
//! the report to selftest.log in the data folder (useful to troubleshoot a friend's PC).

use crate::{audio, claude, coach, knowledge, models, screen, settings::Settings, stt, text, tts, wow};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

struct Report {
    text: String,
    file: Option<std::fs::File>,
}

impl Report {
    fn line(&mut self, line: impl AsRef<str>) {
        let line = line.as_ref();
        let _ = writeln!(self.text, "{line}");
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
        println!("{line}");
    }
}

pub fn run(data: &Path, addon: &Path, ask_claude: bool) -> bool {
    let _ = std::fs::create_dir_all(data);
    let mut report = Report { text: String::new(), file: std::fs::File::create(data.join("selftest.log")).ok() };
    let settings = Settings::load(&data.join("settings.json"));
    let models_dir = data.join("models");
    let mut ok = true;
    report.line(format!("Mentor {} self-test", env!("CARGO_PKG_VERSION")));

    let status = claude::auth_status(&settings.claude_executable);
    report.line(format!(
        "[claude] installed={} logged_in={} method={:?} plan={:?} error={:?}",
        status.installed, status.logged_in, status.method, status.plan, status.error
    ));

    let progress = |label: &str| {
        let label = label.to_string();
        let mut last = 0u64;
        move |done: u64, total: u64| {
            if total > 0 && (done * 10 / total) != last {
                last = done * 10 / total;
                println!("  {label}: {} %", done * 100 / total);
            }
        }
    };
    if !models::whisper_ready(&models_dir, &settings.whisper_model) {
        report.line("[models] downloading speech recognition model...");
        if let Err(error) = models::download_whisper(&models_dir, &settings.whisper_model, &mut progress("whisper")) {
            report.line(format!("[models] FAILED whisper: {error:#}"));
            ok = false;
        }
    }
    if !models::voice_ready(&models_dir, &settings.voice) {
        report.line("[models] downloading voice...");
        if let Err(error) = models::download_voice(&models_dir, &settings.voice, &mut progress("voice")) {
            report.line(format!("[models] FAILED voice: {error:#}"));
            ok = false;
        }
    }

    let started = Instant::now();
    let speaker = match tts::Speaker::load(&models::voice_dir(&models_dir, &settings.voice), settings.voice_speed) {
        Ok(speaker) => {
            report.line(format!("[tts] voice loaded in {:.1} s", started.elapsed().as_secs_f32()));
            Some(speaker)
        }
        Err(error) => {
            report.line(format!("[tts] FAILED: {error:#}"));
            ok = false;
            None
        }
    };

    let started = Instant::now();
    let path = models::whisper_path(&models_dir, &settings.whisper_model);
    let transcriber = stt::Transcriber::load(&path, settings.use_gpu, &settings.language, &settings.initial_prompt)
        .or_else(|error| {
            report.line(format!("[stt] GPU load failed ({error:#}), trying CPU"));
            stt::Transcriber::load(&path, false, &settings.language, &settings.initial_prompt)
        });
    let transcriber = match transcriber {
        Ok(transcriber) => {
            report.line(format!("[stt] {} loaded in {:.1} s", transcriber.description, started.elapsed().as_secs_f32()));
            Some(transcriber)
        }
        Err(error) => {
            report.line(format!("[stt] FAILED: {error:#}"));
            ok = false;
            None
        }
    };

    if let (Some(speaker), Some(transcriber)) = (&speaker, &transcriber) {
        let sentence = "Quelle arme dois-je équiper sur mon démoniste ?";
        let started = Instant::now();
        match speaker.synthesize(sentence) {
            Ok((samples, rate)) => {
                let synthesis = started.elapsed();
                let audio16 = audio::resample(&samples, rate, audio::CAPTURE_RATE);
                let started = Instant::now();
                match transcriber.transcribe(&audio16) {
                    Ok(heard) => report.line(format!(
                        "[voice round trip] said \"{sentence}\" ({:.2} s) -> heard \"{heard}\" ({:.2} s)",
                        synthesis.as_secs_f32(),
                        started.elapsed().as_secs_f32()
                    )),
                    Err(error) => {
                        report.line(format!("[stt] FAILED transcription: {error:#}"));
                        ok = false;
                    }
                }
            }
            Err(error) => {
                report.line(format!("[tts] FAILED synthesis: {error:#}"));
                ok = false;
            }
        }
    }

    let (inputs, outputs) = audio::list_devices();
    report.line(format!("[audio] {} microphone(s), {} output(s)", inputs.len(), outputs.len()));
    match audio::Recorder::open(&settings.input_device) {
        Ok(_) => report.line("[audio] microphone opened"),
        Err(error) => {
            report.line(format!("[audio] FAILED microphone: {error:#}"));
            ok = false;
        }
    }

    let started = Instant::now();
    match screen::capture(settings.screenshot_max_width) {
        Ok(shot) => report.line(format!(
            "[screen] {}x{} jpeg {} KB in {:.2} s{}",
            shot.width,
            shot.height,
            shot.jpeg.len() / 1024,
            started.elapsed().as_secs_f32(),
            if screen::is_mostly_black(&shot) { " (BLACK: use windowed fullscreen)" } else { "" }
        )),
        Err(error) => {
            report.line(format!("[screen] FAILED: {error:#}"));
            ok = false;
        }
    }

    let folder = if settings.wow_folder.is_empty() { wow::detect_default_folder().unwrap_or_default() } else { settings.wow_folder.clone() };
    let status = wow::status(&folder, addon);
    report.line(format!(
        "[wow] folder={} version={:?} interface={:?} addon_installed={:?} bundled={} outdated={}",
        folder,
        status.flavor.as_ref().map(|flavor| flavor.version.clone()),
        status.flavor.as_ref().and_then(|flavor| flavor.interface),
        status.addon_installed,
        status.addon_bundled,
        status.addon_outdated
    ));
    let sheet = match wow::load_snapshot(&folder) {
        Ok(snapshot) => {
            let sheet = wow::format_snapshot(&snapshot);
            report.line(format!("[wow] character {} ({} characters of context)", snapshot.key, sheet.len()));
            Some(sheet)
        }
        Err(error) => {
            report.line(format!("[wow] no character yet: {error:#}"));
            None
        }
    };

    let knowledge = knowledge::Knowledge::open(&data.join("claude"), data.join("knowledge.json"));
    if coach::knowledge_active(&settings) {
        let started = Instant::now();
        if let Some(sync) = knowledge.sync(&settings.language) {
            report.line(format!(
                "[knowledge] {} official document(s), {} change(s) in {:.0} s",
                sync.total,
                sync.changes.len(),
                started.elapsed().as_secs_f32()
            ));
            for error in &sync.errors {
                report.line(format!("[knowledge] warning: {error}"));
            }
            if sync.total == 0 {
                report.line("[knowledge] FAILED: no official document could be fetched");
                ok = false;
            }
        }
    } else {
        report.line("[knowledge] disabled");
    }

    if ask_claude && status_ok_for_claude(&settings) {
        let work_dir = data.join("selftest-claude");
        let _ = std::fs::remove_dir_all(&work_dir);
        let prompt = coach::prompt_for(&settings, &coach::knowledge_section(&settings, &knowledge));
        let options = coach::claude_options(&settings);
        report.line(format!("[claude] mode: {}", if options.api_key.is_some() { "API key" } else { "subscription" }));
        match claude::ClaudeBackend::new(work_dir.clone(), &prompt, options) {
            Ok(backend) => {
                copy_files(&data.join("claude").join(knowledge::FOLDER), &work_dir.join(knowledge::FOLDER));
                let question = "Sans chercher sur le web : rappelle ma classe et mon niveau en une phrase, puis donne-moi une macro très simple pour lancer Trait de l'ombre sur ma cible.";
                let text_message = match &sheet {
                    Some(sheet) => format!("{sheet}\n\n---\nMy question: {question}"),
                    None => question.to_string(),
                };
                ok &= ask(&backend, &mut report, "claude", &text_message);
                if coach::knowledge_active(&settings) {
                    let question = "Sans chercher sur le web : d'après ta base de connaissances locale, de quand datent les dernières notes de correctifs officielles ? Cite un changement précis qu'elles contiennent.";
                    ok &= ask(&backend, &mut report, "claude + knowledge", question);
                }
                backend.close();
            }
            Err(error) => {
                report.line(format!("[claude] FAILED: {error:#}"));
                ok = false;
            }
        }
    }
    report.line(if ok { "RESULT: OK" } else { "RESULT: some checks failed" });
    ok
}

/// Asks one question and writes the answer and its spoken part to the report.
fn ask(backend: &claude::ClaudeBackend, report: &mut Report, label: &str, text_message: &str) -> bool {
    let message = claude::ClaudeBackend::build_message(text_message, None);
    let cancel = AtomicBool::new(false);
    let mut filter = text::SpeechFilter::default();
    let mut splitter = text::SentenceSplitter::default();
    let mut spoken = Vec::new();
    let started = Instant::now();
    let mut first: Option<f32> = None;
    let mut on_text = |chunk: &str| {
        for sentence in splitter.feed(&filter.push(chunk)) {
            first.get_or_insert(started.elapsed().as_secs_f32());
            spoken.push(sentence);
        }
    };
    let result = backend.ask(&message, &cancel, &mut on_text);
    spoken.extend(splitter.feed(&filter.finish()));
    spoken.extend(splitter.flush());
    match result {
        Ok(answer) => {
            report.line(format!("[{label}] answer in {:.1} s (first sentence {:.1} s):", started.elapsed().as_secs_f32(), first.unwrap_or(0.0)));
            for line in answer.lines() {
                report.line(format!("    | {line}"));
            }
            report.line(format!("[{label}] spoken part:"));
            for sentence in spoken {
                report.line(format!("    > {sentence}"));
            }
            true
        }
        Err(error) => {
            report.line(format!("[{label}] FAILED: {error:#}"));
            false
        }
    }
}

/// Copies the files of a folder (not its subfolders).
fn copy_files(source: &Path, target: &Path) {
    let _ = std::fs::create_dir_all(target);
    for entry in std::fs::read_dir(source).into_iter().flatten().flatten() {
        if entry.path().is_file() {
            let _ = std::fs::copy(entry.path(), target.join(entry.file_name()));
        }
    }
}

fn status_ok_for_claude(settings: &Settings) -> bool {
    if settings.uses_api_key() {
        return settings.api_key().is_some();
    }
    claude::auth_status(&settings.claude_executable).logged_in
}
