//! Long-lived `claude -p` process in stream-json mode, with a persistent session.
//! Each player uses their own Claude Code login (claude.ai subscription) or API key.
//!
//! Claude gets the web tools plus read-only file tools. In the `dontAsk` permission mode
//! Claude Code only lets it read inside the directory it runs in, which holds nothing but
//! the prompt and the knowledge base: every other file on the computer is refused.

use anyhow::{anyhow, Result};
use base64::Engine;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TOOLS: &str = "WebSearch,WebFetch,Read,Grep,Glob";
/// Tools allowed everywhere. The file tools are deliberately left out: without a rule they
/// only work inside the working directory.
const ALLOWED_TOOLS: &str = "WebSearch,WebFetch";
/// A conversation left idle longer than this is not resumed. A new play session starts
/// fresh, which keeps answers fast and avoids the long pause of a history compaction.
const SESSION_MAX_IDLE: Duration = Duration::from_secs(12 * 3600);
const STRIPPED_ENV: [&str; 3] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "OPENAI_API_KEY"];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
const BLOCK_BREAK: &str = "\n";
static SESSION_MISSING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)no conversation found|session[^\n]*not found|could not (?:find|resume)").unwrap());

pub fn hide_window(command: &mut Command, flags: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = (command, flags);
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

pub fn find_executable(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let path = PathBuf::from(configured.trim());
        return path.is_file().then_some(path);
    }
    let known = home().join(".local").join("bin").join("claude.exe");
    if known.is_file() {
        return Some(known);
    }
    let path_var = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_var) {
        for name in ["claude.exe", "claude"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn child_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    for key in STRIPPED_ENV {
        command.env_remove(key);
    }
    command
}

/// Checks an Anthropic API key with the free models listing endpoint.
pub fn check_api_key(key: &str) -> Result<()> {
    let key = key.trim();
    if !key.starts_with("sk-ant-") {
        return Err(anyhow!("ce n'est pas une clé API Anthropic (elle commence par sk-ant-)"));
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?
        .get("https://api.anthropic.com/v1/models?limit=1")
        .header("x-api-key", key)
        .header("anthropic-version", "2026-10-21")
        .send()
        .map_err(|error| anyhow!("impossible de joindre l'API Anthropic : {error}"))?;
    match response.status().as_u16() {
        200 => Ok(()),
        401 => Err(anyhow!("clé refusée : elle est invalide ou a été supprimée")),
        403 => Err(anyhow!("clé sans permission : vérifie ton compte sur la console Anthropic")),
        429 => Err(anyhow!("trop de requêtes, réessaie dans une minute")),
        code => Err(anyhow!("l'API Anthropic a répondu {code}")),
    }
}

fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/F", "/T", "/PID", &child.id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window(&mut command, CREATE_NO_WINDOW);
        let _ = command.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct AuthStatus {
    pub installed: bool,
    pub executable: Option<String>,
    pub logged_in: bool,
    pub method: Option<String>,
    pub plan: Option<String>,
    pub email: Option<String>,
    pub error: Option<String>,
}

pub fn auth_status(configured: &str) -> AuthStatus {
    let Some(executable) = find_executable(configured) else {
        return AuthStatus::default();
    };
    let mut status = AuthStatus { installed: true, executable: Some(executable.to_string_lossy().into()), ..Default::default() };
    let mut command = child_command(&executable);
    command.args(["auth", "status"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_window(&mut command, CREATE_NO_WINDOW);
    let output = match run_with_timeout(command, Duration::from_secs(20)) {
        Ok(output) => output,
        Err(error) => {
            status.error = Some(error.to_string());
            return status;
        }
    };
    match serde_json::from_str::<Value>(&output) {
        Ok(value) => {
            status.logged_in = value.get("loggedIn").and_then(Value::as_bool).unwrap_or(false);
            status.method = value.get("authMethod").and_then(Value::as_str).map(str::to_string);
            status.plan = value.get("subscriptionType").and_then(Value::as_str).map(str::to_string);
            status.email = value.get("email").and_then(Value::as_str).map(str::to_string);
        }
        Err(_) => status.error = Some(output.lines().take(3).collect::<Vec<_>>().join(" ")),
    }
    status
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<String> {
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() > deadline {
            kill_tree(&mut child);
            return Err(anyhow!("claude n'a pas répondu à temps"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(reader.join().unwrap_or_default())
}

/// Opens a visible PowerShell window, so the player can follow the installer or the login.
pub fn open_console(script: &str) -> Result<()> {
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script]);
    for key in STRIPPED_ENV {
        command.env_remove(key);
    }
    hide_window(&mut command, CREATE_NEW_CONSOLE);
    command.spawn()?;
    Ok(())
}

pub fn install_claude() -> Result<()> {
    open_console(
        "Write-Host 'Installation de Claude Code...' -ForegroundColor Cyan; \
         irm https://claude.ai/install.ps1 | iex; \
         Write-Host ''; Write-Host 'Terminé. Tu peux fermer cette fenêtre et revenir dans Mentor.' -ForegroundColor Green; \
         Read-Host 'Entrée pour fermer'",
    )
}

pub fn login_claude(configured: &str) -> Result<()> {
    let executable = find_executable(configured).ok_or_else(|| anyhow!("Claude Code n'est pas installé"))?;
    let path = executable.to_string_lossy().replace('\'', "''");
    open_console(&format!(
        "Write-Host 'Connexion à ton compte Claude (abonnement Pro ou Max)...' -ForegroundColor Cyan; \
         & '{path}' auth login --claudeai; \
         Write-Host ''; Write-Host 'Tu peux fermer cette fenêtre et revenir dans Mentor.' -ForegroundColor Green; \
         Read-Host 'Entrée pour fermer'"
    ))
}

pub fn logout_claude(configured: &str) -> Result<()> {
    let executable = find_executable(configured).ok_or_else(|| anyhow!("Claude Code n'est pas installé"))?;
    let mut command = child_command(&executable);
    command.args(["auth", "logout"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    hide_window(&mut command, CREATE_NO_WINDOW);
    run_with_timeout(command, Duration::from_secs(20))?;
    Ok(())
}

#[derive(Default)]
struct StreamParser {
    session_id: Option<String>,
    result_text: Option<String>,
    is_error: bool,
    turn_done: bool,
    streamed: String,
}

impl StreamParser {
    fn emitted(&self) -> bool {
        !self.streamed.trim().is_empty()
    }

    fn text(&self) -> String {
        if self.emitted() {
            self.streamed.trim().to_string()
        } else {
            self.result_text.clone().unwrap_or_default().trim().to_string()
        }
    }

    fn feed_line(&mut self, line: &str) -> Vec<String> {
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            return Vec::new();
        };
        if let Some(session) = message.get("session_id").and_then(Value::as_str).filter(|session| !session.is_empty()) {
            self.session_id = Some(session.to_string());
        }
        match message.get("type").and_then(Value::as_str) {
            Some("stream_event") if message.get("parent_tool_use_id").is_none_or(Value::is_null) => {
                let event = message.get("event").cloned().unwrap_or(Value::Null);
                match event.get("type").and_then(Value::as_str) {
                    Some("content_block_delta") => {
                        let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                        if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                            if let Some(text) = delta.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()) {
                                self.streamed.push_str(text);
                                return vec![text.to_string()];
                            }
                        }
                    }
                    Some("content_block_stop") | Some("message_stop") => {
                        if !self.streamed.is_empty() && !self.streamed.ends_with(BLOCK_BREAK) {
                            self.streamed.push_str(BLOCK_BREAK);
                        }
                        return vec![BLOCK_BREAK.to_string()];
                    }
                    _ => {}
                }
            }
            Some("result") => {
                let subtype = message.get("subtype").and_then(Value::as_str);
                self.is_error = message.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype.is_some_and(|kind| kind != "success");
                self.result_text = message.get("result").and_then(Value::as_str).map(str::to_string);
                self.turn_done = true;
            }
            _ => {}
        }
        Vec::new()
    }
}

struct ClaudeProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Option<String>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    resumed: bool,
}

impl ClaudeProcess {
    fn spawn(executable: &Path, args: &[String], work_dir: &Path, resumed: bool, api_key: Option<&str>) -> Result<Self> {
        let mut command = child_command(executable);
        if let Some(key) = api_key {
            command.env("ANTHROPIC_API_KEY", key);
        }
        command
            .args(args)
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_window(&mut command, CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        let mut child = command.spawn().map_err(|error| anyhow!("impossible de lancer claude: {error}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        let (sender, lines) = unbounded();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(Some(line)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = sender.send(None);
        });
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_sink = stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                let mut buffer = stderr_sink.lock();
                buffer.push_back(line);
                if buffer.len() > 200 {
                    buffer.pop_front();
                }
            }
        });
        Ok(Self { child, stdin, lines, stderr, resumed })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, payload: &Value) -> std::io::Result<()> {
        let mut data = serde_json::to_vec(payload)?;
        data.push(b'\n');
        self.stdin.write_all(&data)?;
        self.stdin.flush()
    }

    fn stderr_tail(&self) -> String {
        let buffer = self.stderr.lock();
        buffer.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join("\n")
    }

    fn close(mut self) {
        drop(self.stdin);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        kill_tree(&mut self.child);
    }

    fn kill(mut self) {
        kill_tree(&mut self.child);
    }
}

#[derive(Clone, PartialEq)]
pub struct ClaudeOptions {
    pub executable: String,
    pub model: String,
    pub effort: String,
    /// Set in pay-as-you-go mode: Claude Code then bills the API key instead of the subscription.
    pub api_key: Option<String>,
}

struct Inner {
    process: Option<ClaudeProcess>,
    session_id: Option<String>,
    /// Last time the conversation was used.
    last_used: SystemTime,
    options: ClaudeOptions,
}

pub struct ClaudeBackend {
    work_dir: PathBuf,
    prompt_file: PathBuf,
    session_file: PathBuf,
    inner: Mutex<Inner>,
    /// Prompt and options to apply before the next turn. Kept apart from `inner` so that
    /// changing them never waits for the answer being generated.
    pending: Mutex<Option<(String, ClaudeOptions)>>,
    /// Number of conversations started from scratch: tells the coach when Claude no longer
    /// knows what was sent earlier (the character sheet).
    generation: AtomicU64,
    counter: AtomicU64,
    timeout: Duration,
    interrupt_grace: Duration,
}

enum TurnEnd {
    Done,
    TimedOut,
    Exited(String),
    Abandoned,
}

impl ClaudeBackend {
    pub fn new(work_dir: PathBuf, prompt: &str, options: ClaudeOptions) -> Result<Self> {
        std::fs::create_dir_all(&work_dir)?;
        let prompt_file = work_dir.join("coach_prompt.md");
        std::fs::write(&prompt_file, prompt)?;
        let session_file = work_dir.join("session.json");
        let saved = std::fs::read_to_string(&session_file).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let session_id = saved.as_ref().and_then(|value| value.get("session_id").and_then(Value::as_str).map(str::to_string));
        let last_used = saved
            .as_ref()
            .and_then(|value| value.get("updated").and_then(Value::as_f64))
            .map(|seconds| UNIX_EPOCH + Duration::from_secs_f64(seconds.max(0.0)))
            .unwrap_or(UNIX_EPOCH);
        Ok(Self {
            work_dir,
            prompt_file,
            session_file,
            inner: Mutex::new(Inner { process: None, session_id, last_used, options }),
            pending: Mutex::new(None),
            generation: AtomicU64::new(0),
            counter: AtomicU64::new(1),
            timeout: Duration::from_secs(180),
            interrupt_grace: Duration::from_secs(5),
        })
    }

    /// Number of the current conversation: it changes when Claude starts from scratch.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// New prompt and options, used from the next turn on. Returns immediately, even while
    /// an answer is being generated; call `warm_up` afterwards to restart Claude ahead of time.
    pub fn configure(&self, prompt: &str, options: ClaudeOptions) {
        *self.pending.lock() = Some((prompt.to_string(), options));
    }

    fn apply_pending(&self, inner: &mut Inner) {
        if let Some((prompt, options)) = self.pending.lock().take() {
            if let Err(error) = std::fs::write(&self.prompt_file, prompt) {
                log::warn!("Could not write the prompt: {error}");
            }
            inner.options = options;
            if let Some(process) = inner.process.take() {
                process.close();
            }
        }
    }

    fn forget_session(&self, inner: &mut Inner) {
        if let Some(process) = inner.process.take() {
            process.kill();
        }
        inner.session_id = None;
        let _ = std::fs::remove_file(&self.session_file);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Drops a conversation that was left idle for too long. Returns true when it did.
    fn expire_session(&self, inner: &mut Inner) -> bool {
        let idle = SystemTime::now().duration_since(inner.last_used).unwrap_or_default();
        if inner.session_id.is_none() || idle < SESSION_MAX_IDLE {
            return false;
        }
        log::info!("Conversation idle for {} h: starting a new one", idle.as_secs() / 3600);
        self.forget_session(inner);
        true
    }

    /// Starts Claude ahead of the next question (waits for the answer in progress, if any).
    pub fn warm_up(&self) {
        let mut inner = self.inner.lock();
        self.apply_pending(&mut inner);
        self.expire_session(&mut inner);
        if let Err(error) = self.ensure_process(&mut inner) {
            log::warn!("Could not start claude: {error}");
        }
    }

    /// To call before composing a question: applies the idle policy and returns the number
    /// of the conversation the question will go to.
    pub fn begin_turn(&self) -> u64 {
        let mut inner = self.inner.lock();
        self.apply_pending(&mut inner);
        self.expire_session(&mut inner);
        self.generation()
    }

    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        self.apply_pending(&mut inner);
        self.forget_session(&mut inner);
        drop(inner);
        self.warm_up();
    }

    pub fn close(&self) {
        if let Some(process) = self.inner.lock().process.take() {
            process.close();
        }
    }

    fn build_args(&self, options: &ClaudeOptions, resume: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--tools",
            TOOLS,
            "--allowedTools",
            ALLOWED_TOOLS,
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--disable-slash-commands",
        ]
        .iter()
        .map(|argument| argument.to_string())
        .collect();
        args.push("--append-system-prompt-file".into());
        args.push(self.prompt_file.to_string_lossy().into());
        if !options.model.is_empty() {
            args.extend(["--model".into(), options.model.clone()]);
        }
        if !options.effort.is_empty() {
            args.extend(["--effort".into(), options.effort.clone()]);
        }
        if let Some(session) = resume {
            args.extend(["--resume".into(), session.to_string()]);
        }
        args
    }

    fn ensure_process<'a>(&self, inner: &'a mut Inner) -> Result<&'a mut ClaudeProcess> {
        let alive = inner.process.as_mut().is_some_and(ClaudeProcess::alive);
        if !alive {
            if let Some(process) = inner.process.take() {
                process.kill();
            }
            let executable = find_executable(&inner.options.executable).ok_or_else(|| anyhow!("Claude Code n'est pas installé"))?;
            let resume = inner.session_id.clone();
            let args = self.build_args(&inner.options, resume.as_deref());
            log::debug!("Starting claude {args:?}");
            let api_key = inner.options.api_key.clone();
            inner.process = Some(ClaudeProcess::spawn(&executable, &args, &self.work_dir, resume.is_some(), api_key.as_deref())?);
        }
        inner.process.as_mut().ok_or_else(|| anyhow!("claude unavailable"))
    }

    fn save_session(&self, inner: &mut Inner, session: &str) {
        inner.session_id = Some(session.to_string());
        inner.last_used = SystemTime::now();
        let updated = inner.last_used.duration_since(UNIX_EPOCH).map(|duration| duration.as_secs_f64()).unwrap_or_default();
        let _ = std::fs::write(&self.session_file, json!({ "session_id": session, "updated": updated }).to_string());
    }

    pub fn build_message(text: &str, image: Option<&[u8]>) -> Value {
        let content = match image {
            Some(jpeg) => json!([
                { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(jpeg) } },
                { "type": "text", "text": format!("A screenshot of my current game screen is attached.\n\n{text}") }
            ]),
            None => Value::String(text.to_string()),
        };
        json!({ "type": "user", "message": { "role": "user", "content": content }, "parent_tool_use_id": null, "session_id": "default" })
    }

    pub fn ask(&self, message: &Value, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<String> {
        let mut inner = self.inner.lock();
        self.apply_pending(&mut inner);
        let (mut parser, mut end, mut resumed) = self.run_turn(&mut inner, message, cancel, on_text)?;
        let details = format!("{}\n{}", parser.result_text.clone().unwrap_or_default(), match &end {
            TurnEnd::Exited(stderr) => stderr.clone(),
            _ => String::new(),
        });
        if !cancel.load(Ordering::SeqCst) && !parser.emitted() && resumed && SESSION_MISSING.is_match(&details) {
            log::warn!("Session could not be resumed, starting a new one");
            self.forget_session(&mut inner);
            (parser, end, resumed) = self.run_turn(&mut inner, message, cancel, on_text)?;
        }
        let _ = resumed;
        let exited = matches!(end, TurnEnd::Exited(_));
        if let Some(session) = parser.session_id.clone() {
            if !(parser.is_error && exited) {
                self.save_session(&mut inner, &session);
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Ok(parser.text());
        }
        match end {
            TurnEnd::TimedOut => return Err(anyhow!("Claude n'a pas répondu en {} secondes", self.timeout.as_secs())),
            TurnEnd::Exited(stderr) if !parser.turn_done => {
                return Err(anyhow!(if stderr.is_empty() { "claude s'est arrêté avant la fin de sa réponse".to_string() } else { stderr }))
            }
            _ => {}
        }
        if parser.emitted() {
            return Ok(parser.text());
        }
        if let Some(result) = parser.result_text.clone().filter(|_| !parser.is_error) {
            on_text(&result);
            return Ok(result.trim().to_string());
        }
        Err(anyhow!(parser.result_text.unwrap_or_else(|| "claude s'est arrêté sans répondre".into())))
    }

    fn run_turn(&self, inner: &mut Inner, message: &Value, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<(StreamParser, TurnEnd, bool)> {
        let process = self.ensure_process(inner)?;
        let resumed = process.resumed;
        let mut parser = StreamParser::default();
        if let Err(error) = process.send(message) {
            log::warn!("claude stdin closed: {error}");
        }
        let deadline = Instant::now() + self.timeout;
        let mut interrupt_deadline: Option<Instant> = None;
        loop {
            let process = inner.process.as_mut().ok_or_else(|| anyhow!("claude unavailable"))?;
            if cancel.load(Ordering::SeqCst) && interrupt_deadline.is_none() {
                let request_id = format!("req_{}", self.counter.fetch_add(1, Ordering::SeqCst));
                let _ = process.send(&json!({ "type": "control_request", "request_id": request_id, "request": { "subtype": "interrupt" } }));
                interrupt_deadline = Some(Instant::now() + self.interrupt_grace);
            }
            match process.lines.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(line)) => {
                    for chunk in parser.feed_line(&line) {
                        if !cancel.load(Ordering::SeqCst) {
                            on_text(&chunk);
                        }
                    }
                    if parser.turn_done {
                        return Ok((parser, TurnEnd::Done, resumed));
                    }
                }
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(200));
                    let stderr = process.stderr_tail();
                    if let Some(process) = inner.process.take() {
                        process.kill();
                    }
                    return Ok((parser, TurnEnd::Exited(stderr), resumed));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if interrupt_deadline.is_some_and(|limit| now > limit) {
                        log::warn!("claude did not stop the turn in time, restarting it");
                        if let Some(process) = inner.process.take() {
                            process.kill();
                        }
                        return Ok((parser, TurnEnd::Abandoned, resumed));
                    }
                    if now > deadline {
                        if let Some(process) = inner.process.take() {
                            process.kill();
                        }
                        return Ok((parser, TurnEnd::TimedOut, resumed));
                    }
                }
            }
        }
    }
}

/// `game_notes` says what is special about the game; `knowledge` describes the local
/// knowledge base (empty when there is none).
pub fn render_prompt(template: &str, game: &str, language: &str, sources: &[String], game_notes: &str, knowledge: &str) -> String {
    let language_name = match language {
        "fr" => "French",
        "en" => "English",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        other => other,
    };
    let sources = if sources.is_empty() { "official sites and well-known community wikis and guide sites".to_string() } else { sources.join(", ") };
    let knowledge = if knowledge.trim().is_empty() {
        "Knowledge base\n- No local knowledge base is available right now: rely on web searches for game facts.".to_string()
    } else {
        knowledge.trim().to_string()
    };
    template
        .replace("{{GAME_NOTES}}", game_notes.trim())
        .replace("{{KNOWLEDGE}}", &knowledge)
        .replace("{{GAME}}", game)
        .replace("{{LANGUAGE}}", language_name)
        .replace("{{SOURCES}}", &sources)
}
