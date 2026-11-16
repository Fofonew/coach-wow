import argparse
import asyncio
import base64
import itertools
import json
import logging
import os
import queue
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import tomllib
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import numpy as np

APP_DIR = Path(__file__).resolve().parent
CAPTURE_RATE = 16000
PLAYBACK_RATE = 48000
PLAYBACK_BLOCK = 960
IS_WINDOWS = os.name == "nt"
ACTIONS = ("push_to_talk", "push_to_talk_with_screenshot", "screenshot", "stop", "new_session")
DEFAULT_SCREEN_KEYWORDS = (
    "écran", "ecran", "screen", "regarde", "tu vois", "vois-tu", "ce que je vois", "capture",
    "cet objet", "cet item", "cette quête", "ce mob", "ce boss", "ce pnj", "ce sort", "cette carte",
    "infobulle", "tooltip", "c'est quoi ça", "c'est quoi ce", "c'est quoi cette", "ça c'est quoi",
)
RECORDING_ACTIONS = ("push_to_talk", "push_to_talk_with_screenshot")
LANGUAGE_NAMES = {"fr": "French", "en": "English", "de": "German", "es": "Spanish", "it": "Italian"}
STRIPPED_ENV_KEYS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "OPENAI_API_KEY")
CLAUDE_TOOLS = "WebSearch,WebFetch"
BLOCK_BREAK = "\n"
SESSION_MISSING_PATTERN = re.compile(r"no conversation found|session[^\n]*not found|could not (?:find|resume)", re.IGNORECASE)
HALLUCINATION_PATTERN = re.compile(r"amara\.org|sous-titr|merci d'avoir regard|abonnez-vous", re.IGNORECASE)
SENTENCE_BOUNDARY = re.compile(r"(?<=[.!?…])\s+|\n+")
MARKDOWN_LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")
URL_PATTERN = re.compile(r"https?://\S+|www\.\S+")
LIST_MARKER = re.compile(r"^\s*(?:[-*•]|\d+[.)])\s+", re.MULTILINE)
MARKDOWN_SYMBOLS = re.compile(r"[*_`#>|~]+")
WHITESPACE = re.compile(r"\s+")
WORD_CHARACTER = re.compile(r"\w")

MESSAGES = {
    "fr": {
        "ready": "Coach prêt.",
        "new_session": "Nouvelle session.",
        "screenshot_armed": "Capture prise.",
        "backend_error": "Je n'ai pas pu obtenir de réponse, regarde la console.",
    },
    "en": {
        "ready": "Coach ready.",
        "new_session": "New session.",
        "screenshot_armed": "Screenshot taken.",
        "backend_error": "I could not get an answer, check the console.",
    },
}

logger = logging.getLogger("coach")


class BackendError(RuntimeError):
    pass


def clean_for_speech(text: str) -> str:
    text = MARKDOWN_LINK.sub(r"\1", text)
    text = URL_PATTERN.sub("", text)
    text = LIST_MARKER.sub("", text)
    text = MARKDOWN_SYMBOLS.sub("", text)
    text = WHITESPACE.sub(" ", text)
    return text.strip()


class SentenceSplitter:
    def __init__(self, max_chars: int = 220):
        self.buffer = ""
        self.max_chars = max_chars

    def feed(self, text: str) -> list[str]:
        self.buffer += text
        sentences = []
        while True:
            match = SENTENCE_BOUNDARY.search(self.buffer)
            if match:
                piece = self.buffer[: match.start()]
                self.buffer = self.buffer[match.end():]
            elif len(self.buffer) > self.max_chars:
                cut = self.buffer.rfind(", ", 0, self.max_chars)
                if cut == -1:
                    cut = self.buffer.rfind(" ", 0, self.max_chars)
                if cut == -1:
                    cut = self.max_chars - 1
                piece = self.buffer[: cut + 1]
                self.buffer = self.buffer[cut + 1:]
            else:
                break
            self._append(sentences, piece)
        return sentences

    def flush(self) -> list[str]:
        sentences = []
        self._append(sentences, self.buffer)
        self.buffer = ""
        return sentences

    @staticmethod
    def _append(sentences: list[str], piece: str) -> None:
        cleaned = clean_for_speech(piece)
        if cleaned and WORD_CHARACTER.search(cleaned):
            sentences.append(cleaned)


def resample(samples: np.ndarray, source_rate: int, target_rate: int) -> np.ndarray:
    samples = np.asarray(samples, dtype=np.float32).reshape(-1)
    if source_rate == target_rate or samples.size < 2:
        return samples
    target_length = int(round(samples.size * target_rate / source_rate))
    source_positions = np.arange(samples.size, dtype=np.float64)
    target_positions = np.linspace(0, samples.size - 1, target_length)
    return np.interp(target_positions, source_positions, samples).astype(np.float32)


def make_tone(frequencies: list[int], duration: float = 0.07, volume: float = 0.18) -> np.ndarray:
    segments = []
    for frequency in frequencies:
        count = int(PLAYBACK_RATE * duration)
        timeline = np.arange(count) / PLAYBACK_RATE
        wave = np.sin(2 * np.pi * frequency * timeline) * volume
        ramp = min(count // 4, int(PLAYBACK_RATE * 0.006))
        if ramp > 0:
            envelope = np.ones(count)
            envelope[:ramp] = np.linspace(0.0, 1.0, ramp)
            envelope[-ramp:] = np.linspace(1.0, 0.0, ramp)
            wave = wave * envelope
        segments.append(wave)
        segments.append(np.zeros(int(PLAYBACK_RATE * 0.015)))
    return np.concatenate(segments).astype(np.float32)


TONES = {
    "start": make_tone([660, 880]),
    "stop": make_tone([880, 660]),
    "error": make_tone([300, 220], duration=0.12),
    "reset": make_tone([523, 659, 784]),
}


KNOWN_INSTALL_LOCATIONS = {
    "claude": ("~/.local/bin/claude.exe", "~/.local/bin/claude"),
}
INSTALL_HINTS = {
    "claude": "Install Claude Code from PowerShell with: irm https://claude.ai/install.ps1 | iex\n"
    "Then open a new terminal, run: claude --version, and log in once with: claude",
    "codex": "Install Codex CLI from PowerShell with: irm https://chatgpt.com/codex/install.ps1 | iex\n"
    "Then open a new terminal and log in with: codex login",
}


def resolve_executable(name: str) -> str:
    candidate = Path(name).expanduser()
    if candidate.is_file():
        return str(candidate)
    found = shutil.which(name)
    if found:
        return found
    for location in KNOWN_INSTALL_LOCATIONS.get(name, ()):
        path = Path(location).expanduser()
        if path.is_file():
            return str(path)
    hint = INSTALL_HINTS.get(name, "Set the full path of the executable in coach.toml.")
    raise RuntimeError(f"Executable not found: {name}\n{hint}\nIf it is already installed, set its full path in coach.toml, for example: executable = '~/.local/bin/{name}.exe'")


def child_environment() -> dict[str, str]:
    environment = dict(os.environ)
    for key in STRIPPED_ENV_KEYS:
        environment.pop(key, None)
    return environment


def popen_options() -> dict:
    if IS_WINDOWS:
        return {"creationflags": subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def kill_process_tree(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    if IS_WINDOWS:
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(process.pid)],
            capture_output=True,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        return
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


class ManagedProcess:
    def __init__(self, command: list[str], work_dir: Path, stdin_text: str, cancel_event: threading.Event, timeout_seconds: float):
        self.process = subprocess.Popen(
            command,
            cwd=str(work_dir),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=child_environment(),
            **popen_options(),
        )
        self.cancel_event = cancel_event
        self.timeout_seconds = timeout_seconds
        self.timed_out = False
        self.stderr_lines: list[str] = []
        self.stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self.stderr_thread.start()
        self.watch_thread = threading.Thread(target=self._watch, daemon=True)
        self.watch_thread.start()
        self._send_input(stdin_text)

    def _send_input(self, text: str) -> None:
        try:
            self.process.stdin.write(text.encode("utf-8"))
            self.process.stdin.close()
        except OSError:
            pass

    def _drain_stderr(self) -> None:
        for raw_line in self.process.stderr:
            self.stderr_lines.append(raw_line.decode("utf-8", errors="replace").rstrip())
            if len(self.stderr_lines) > 400:
                del self.stderr_lines[:200]

    def _watch(self) -> None:
        deadline = time.monotonic() + self.timeout_seconds
        while self.process.poll() is None:
            if self.cancel_event.wait(0.1):
                kill_process_tree(self.process)
                return
            if time.monotonic() > deadline:
                self.timed_out = True
                kill_process_tree(self.process)
                return

    def drain_stdout(self) -> None:
        for _ in self.process.stdout:
            pass

    def finish(self) -> int:
        code = self.process.wait()
        self.stderr_thread.join(timeout=2)
        return code

    def stderr_tail(self, count: int = 8) -> str:
        return "\n".join(line for line in self.stderr_lines[-count:] if line)


class ClaudeStreamParser:
    def __init__(self):
        self.session_id: str | None = None
        self.result_text: str | None = None
        self.is_error = False
        self.turn_done = False
        self.streamed: list[str] = []

    @property
    def emitted(self) -> bool:
        return any(chunk.strip() for chunk in self.streamed)

    def text(self) -> str:
        return "".join(self.streamed).strip() or (self.result_text or "").strip()

    def feed_line(self, line: str) -> list[str]:
        line = line.strip()
        if not line:
            return []
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            return []
        if not isinstance(message, dict):
            return []
        session_id = message.get("session_id")
        if isinstance(session_id, str) and session_id:
            self.session_id = session_id
        message_type = message.get("type")
        if message_type == "stream_event" and message.get("parent_tool_use_id") is None:
            return self._handle_event(message.get("event") or {})
        if message_type == "result":
            subtype = message.get("subtype")
            self.is_error = bool(message.get("is_error")) or (subtype is not None and subtype != "success")
            result = message.get("result")
            if isinstance(result, str):
                self.result_text = result
            self.turn_done = True
        return []

    def _handle_event(self, event: dict) -> list[str]:
        event_type = event.get("type")
        if event_type == "content_block_delta":
            delta = event.get("delta") or {}
            if delta.get("type") == "text_delta":
                text = delta.get("text") or ""
                if text:
                    self.streamed.append(text)
                    return [text]
        elif event_type in ("content_block_stop", "message_stop"):
            if self.streamed and not self.streamed[-1].endswith(BLOCK_BREAK):
                self.streamed.append(BLOCK_BREAK)
            return [BLOCK_BREAK]
        return []


class ClaudeProcess:
    def __init__(self, command: list[str], work_dir: Path, resumed: bool):
        self.resumed = resumed
        self.process = subprocess.Popen(
            command,
            cwd=str(work_dir),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=child_environment(),
            **popen_options(),
        )
        self.lines: queue.Queue = queue.Queue()
        self.stderr_lines: list[str] = []
        self.write_lock = threading.Lock()
        self.stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self.stdout_thread.start()
        self.stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self.stderr_thread.start()

    def _read_stdout(self) -> None:
        try:
            for raw_line in self.process.stdout:
                self.lines.put(raw_line.decode("utf-8", errors="replace"))
        finally:
            self.lines.put(None)

    def _read_stderr(self) -> None:
        for raw_line in self.process.stderr:
            self.stderr_lines.append(raw_line.decode("utf-8", errors="replace").rstrip())
            if len(self.stderr_lines) > 400:
                del self.stderr_lines[:200]

    def alive(self) -> bool:
        return self.process.poll() is None

    def send(self, payload: dict) -> None:
        data = (json.dumps(payload, ensure_ascii=False) + "\n").encode("utf-8")
        with self.write_lock:
            self.process.stdin.write(data)
            self.process.stdin.flush()

    def next_line(self, timeout: float) -> str | None:
        return self.lines.get(timeout=timeout)

    def stderr_tail(self, count: int = 8) -> str:
        if not self.alive():
            self.stderr_thread.join(timeout=1)
        return "\n".join(line for line in self.stderr_lines[-count:] if line)

    def close(self) -> None:
        try:
            with self.write_lock:
                self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.kill()

    def kill(self) -> None:
        kill_process_tree(self.process)
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            logger.warning("claude process %s did not exit after kill", self.process.pid)


@dataclass
class ClaudeTurn:
    parser: ClaudeStreamParser
    resumed: bool
    timed_out: bool = False
    process_exited: bool = False
    stderr: str = ""


class ClaudeBackend:
    name = "claude"

    def __init__(self, settings: dict, prompt_text: str, work_dir: Path):
        self.executable = resolve_executable(settings.get("executable", "claude"))
        self.model = str(settings.get("model", ""))
        self.effort = str(settings.get("effort", ""))
        self.isolate = bool(settings.get("isolate", True))
        self.extra_args = [str(item) for item in settings.get("extra_args", [])]
        self.persist_session = bool(settings.get("persist_session", True))
        self.idle_reset_seconds = float(settings.get("session_idle_reset_minutes", 0)) * 60
        self.timeout_seconds = float(settings.get("timeout_seconds", 180))
        self.interrupt_grace_seconds = float(settings.get("interrupt_grace_seconds", 5))
        self.work_dir = work_dir
        self.work_dir.mkdir(parents=True, exist_ok=True)
        self.prompt_file = self.work_dir / "coach_prompt.md"
        self.prompt_file.write_text(prompt_text, encoding="utf-8")
        self.session_file = self.work_dir / "session.json"
        self.lock = threading.RLock()
        self.process: ClaudeProcess | None = None
        self.request_counter = itertools.count(1)
        self.session_id: str | None = self._load_session()
        self.last_activity = time.monotonic()

    def build_command(self, resume_id: str | None) -> list[str]:
        command = [
            self.executable,
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--tools",
            CLAUDE_TOOLS,
            "--allowedTools",
            CLAUDE_TOOLS,
            "--permission-mode",
            "dontAsk",
            "--append-system-prompt-file",
            str(self.prompt_file),
        ]
        if self.model:
            command += ["--model", self.model]
        if self.effort:
            command += ["--effort", self.effort]
        if self.isolate:
            command += ["--setting-sources", "project", "--strict-mcp-config", "--disable-slash-commands"]
        if resume_id:
            command += ["--resume", resume_id]
        command += self.extra_args
        return command

    def build_message(self, question: str, screenshot: Path | None) -> dict:
        content: str | list = question
        if screenshot is not None:
            try:
                encoded = base64.b64encode(screenshot.read_bytes()).decode("ascii")
                content = [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": encoded}},
                    {"type": "text", "text": f"A screenshot of my current game screen is attached.\n\n{question}"},
                ]
            except OSError:
                logger.exception("Could not read screenshot %s", screenshot)
        return {
            "type": "user",
            "message": {"role": "user", "content": content},
            "parent_tool_use_id": None,
            "session_id": "default",
        }

    def warm_up(self) -> None:
        with self.lock:
            try:
                self._ensure_process()
            except Exception:
                logger.exception("Could not start claude")

    def reset(self) -> None:
        with self.lock:
            self._stop_process()
            self._forget_session()
        self.warm_up()

    def close(self) -> None:
        with self.lock:
            self._stop_process()

    def ask(self, question: str, screenshot: Path | None, cancel_event: threading.Event, on_text: Callable[[str], None]) -> str:
        with self.lock:
            if self.session_id and self.idle_reset_seconds > 0 and time.monotonic() - self.last_activity > self.idle_reset_seconds:
                logger.info("Claude session expired after inactivity")
                self._stop_process()
                self._forget_session()
            message = self.build_message(question, screenshot)
            turn = self._run_turn(message, cancel_event, on_text)
            if self._should_retry_fresh(turn, cancel_event):
                logger.warning("Session %s could not be resumed, starting a new one: %s", self.session_id, turn.parser.result_text or turn.stderr)
                self._stop_process()
                self._forget_session()
                turn = self._run_turn(message, cancel_event, on_text)
            self.last_activity = time.monotonic()
            parser = turn.parser
            if parser.session_id and not (parser.is_error and turn.process_exited):
                self.session_id = parser.session_id
                self._save_session()
            if cancel_event.is_set():
                return parser.text()
            if turn.timed_out:
                raise BackendError(f"Claude did not answer within {self.timeout_seconds:.0f} seconds")
            if turn.process_exited and not parser.turn_done:
                raise BackendError(turn.stderr or "claude stopped before finishing its answer")
            if parser.emitted:
                return parser.text()
            if parser.result_text and not parser.is_error:
                on_text(parser.result_text)
                return parser.result_text.strip()
            raise BackendError(parser.result_text or turn.stderr or "claude stopped without answering")

    def _run_turn(self, message: dict, cancel_event: threading.Event, on_text: Callable[[str], None]) -> ClaudeTurn:
        process = self._ensure_process()
        turn = ClaudeTurn(parser=ClaudeStreamParser(), resumed=process.resumed)
        try:
            process.send(message)
        except OSError:
            logger.warning("claude stdin is closed, reading what the process left")
        deadline = time.monotonic() + self.timeout_seconds
        interrupt_deadline: float | None = None
        while True:
            if cancel_event.is_set() and interrupt_deadline is None:
                self._send_interrupt(process)
                interrupt_deadline = time.monotonic() + self.interrupt_grace_seconds
            try:
                line = process.next_line(0.1)
            except queue.Empty:
                now = time.monotonic()
                if interrupt_deadline is not None and now > interrupt_deadline:
                    logger.warning("claude did not stop the turn in time, restarting it")
                    self._discard_process()
                    return turn
                if now > deadline:
                    self._discard_process()
                    turn.timed_out = True
                    return turn
                continue
            if line is None:
                turn.process_exited = True
                turn.stderr = process.stderr_tail()
                self._discard_process()
                return turn
            for chunk in turn.parser.feed_line(line):
                if not cancel_event.is_set():
                    on_text(chunk)
            if turn.parser.turn_done:
                return turn

    def _should_retry_fresh(self, turn: ClaudeTurn, cancel_event: threading.Event) -> bool:
        if cancel_event.is_set() or turn.parser.emitted or not turn.resumed:
            return False
        details = f"{turn.parser.result_text or ''}\n{turn.stderr}"
        return bool(SESSION_MISSING_PATTERN.search(details))

    def _send_interrupt(self, process: ClaudeProcess) -> None:
        request_id = f"req_{next(self.request_counter)}_{os.urandom(4).hex()}"
        try:
            process.send({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}})
        except OSError:
            logger.warning("Could not send the interrupt to claude")

    def _ensure_process(self) -> ClaudeProcess:
        if self.process is not None and self.process.alive():
            return self.process
        if self.process is not None:
            self.process.kill()
        resume_id = self.session_id
        command = self.build_command(resume_id)
        logger.debug("Starting %s", command)
        self.process = ClaudeProcess(command, self.work_dir, resumed=resume_id is not None)
        return self.process

    def _stop_process(self) -> None:
        if self.process is not None:
            self.process.close()
            self.process = None

    def _discard_process(self) -> None:
        if self.process is not None:
            self.process.kill()
            self.process = None

    def _load_session(self) -> str | None:
        if not self.persist_session or not self.session_file.is_file():
            return None
        try:
            data = json.loads(self.session_file.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            logger.warning("Ignoring unreadable session file %s", self.session_file)
            return None
        session_id = data.get("session_id")
        updated = float(data.get("updated", 0))
        if not isinstance(session_id, str) or not session_id:
            return None
        if self.idle_reset_seconds > 0 and time.time() - updated > self.idle_reset_seconds:
            logger.info("Saved session %s is too old, starting a new one", session_id)
            return None
        logger.info("Resuming saved session %s", session_id)
        return session_id

    def _save_session(self) -> None:
        if not self.persist_session or not self.session_id:
            return
        try:
            self.session_file.write_text(json.dumps({"session_id": self.session_id, "updated": time.time()}), encoding="utf-8")
        except OSError:
            logger.exception("Could not save the session file")

    def _forget_session(self) -> None:
        self.session_id = None
        try:
            self.session_file.unlink()
        except FileNotFoundError:
            pass
        except OSError:
            logger.exception("Could not delete the session file")


class CodexBackend:
    name = "codex"

    def __init__(self, settings: dict, prompt_text: str, work_dir: Path):
        self.executable = resolve_executable(settings.get("executable", "codex"))
        self.model = str(settings.get("model", ""))
        self.reasoning_effort = str(settings.get("reasoning_effort", ""))
        self.sandbox = str(settings.get("sandbox", "read-only"))
        self.history_turns = int(settings.get("history_turns", 6))
        self.timeout_seconds = float(settings.get("timeout_seconds", 180))
        self.extra_args = [str(item) for item in settings.get("extra_args", [])]
        self.instructions = prompt_text.strip()
        self.work_dir = work_dir
        self.work_dir.mkdir(parents=True, exist_ok=True)
        self.output_file = self.work_dir / "last_message.txt"
        self.history: list[tuple[str, str]] = []

    def warm_up(self) -> None:
        pass

    def close(self) -> None:
        pass

    def reset(self) -> None:
        self.history.clear()

    def build_prompt(self, question: str, screenshot: Path | None) -> str:
        lines = [self.instructions, ""]
        recent = self.history[-self.history_turns:] if self.history_turns > 0 else []
        if recent:
            lines.append("Recent conversation with the player:")
            for previous_question, previous_answer in recent:
                lines.append(f"Player: {previous_question}")
                lines.append(f"Coach: {previous_answer}")
            lines.append("")
        if screenshot is not None:
            lines.append("A screenshot of the player's current game screen is attached.")
        lines.append(f"Player: {question}")
        return "\n".join(lines)

    def build_command(self, screenshot: Path | None) -> list[str]:
        command = [
            self.executable,
            "exec",
            "--skip-git-repo-check",
            "--ephemeral",
            "--color",
            "never",
            "-C",
            str(self.work_dir),
            "-c",
            "web_search=live",
            "-o",
            str(self.output_file),
        ]
        if self.sandbox:
            command += ["-s", self.sandbox]
        if self.reasoning_effort:
            command += ["-c", f"model_reasoning_effort={self.reasoning_effort}"]
        if self.model:
            command += ["-m", self.model]
        if screenshot is not None:
            command += ["-i", str(screenshot.resolve())]
        command += self.extra_args
        command.append("-")
        return command

    def ask(self, question: str, screenshot: Path | None, cancel_event: threading.Event, on_text: Callable[[str], None]) -> str:
        if self.output_file.exists():
            self.output_file.unlink()
        command = self.build_command(screenshot)
        logger.debug("Running %s", command)
        process = ManagedProcess(command, self.work_dir, self.build_prompt(question, screenshot), cancel_event, self.timeout_seconds)
        process.drain_stdout()
        code = process.finish()
        if cancel_event.is_set():
            return ""
        if process.timed_out:
            raise BackendError(f"Codex did not answer within {self.timeout_seconds:.0f} seconds")
        answer = self.output_file.read_text(encoding="utf-8").strip() if self.output_file.exists() else ""
        if not answer:
            raise BackendError(process.stderr_tail() or f"codex exited with code {code}")
        on_text(answer)
        self.history.append((question, answer))
        if len(self.history) > max(self.history_turns, 1) * 2:
            del self.history[: len(self.history) - max(self.history_turns, 1)]
        return answer


def render_prompt(template: str, general: dict) -> str:
    language = general.get("language", "fr")
    sources = general.get("preferred_sources", [])
    values = {
        "{{GAME}}": general.get("game", "the game"),
        "{{LANGUAGE}}": LANGUAGE_NAMES.get(language, language),
        "{{SOURCES}}": ", ".join(sources) if sources else "official sites and well-known community wikis and guide sites",
    }
    for placeholder, value in values.items():
        template = template.replace(placeholder, value)
    return template


def create_backend(config: dict, base_dir: Path):
    general = config.get("general", {})
    name = str(general.get("backend", "claude")).lower()
    template_path = base_dir / general.get("prompt_file", "coach_prompt.md")
    prompt_text = render_prompt(template_path.read_text(encoding="utf-8"), general)
    runtime_dir = base_dir / general.get("runtime_dir", "runtime")
    if name == "claude":
        return ClaudeBackend(config.get("claude", {}), prompt_text, runtime_dir / "claude")
    if name == "codex":
        return CodexBackend(config.get("codex", {}), prompt_text, runtime_dir / "codex")
    raise ValueError(f"Unknown backend '{name}', expected 'claude' or 'codex'")


def register_nvidia_dlls() -> None:
    if not IS_WINDOWS:
        return
    import site

    roots = [Path(path) / "nvidia" for path in site.getsitepackages() + [site.getusersitepackages()]]
    found = []
    for root in roots:
        if root.is_dir():
            found += [directory for directory in root.glob("*/bin") if directory.is_dir()]
    for directory in found:
        os.add_dll_directory(str(directory))
    if found:
        os.environ["PATH"] = os.pathsep.join(str(directory) for directory in found) + os.pathsep + os.environ.get("PATH", "")


class Transcriber:
    def __init__(self, settings: dict, language: str):
        register_nvidia_dlls()
        from faster_whisper import WhisperModel

        model_name = settings.get("model", "large-v3-turbo")
        device = settings.get("device", "cuda")
        compute_type = settings.get("compute_type", "float16")
        self.language = language
        self.beam_size = int(settings.get("beam_size", 5))
        self.initial_prompt = settings.get("initial_prompt") or None
        try:
            self.model = WhisperModel(model_name, device=device, compute_type=compute_type)
            self.description = f"{model_name} on {device}"
            self.warm_up()
        except Exception as error:
            if device == "cpu":
                raise
            fallback = settings.get("cpu_fallback_model", "small")
            logger.warning("Whisper on %s failed (%s), falling back to %s on CPU", device, error, fallback)
            self.model = WhisperModel(fallback, device="cpu", compute_type="int8")
            self.description = f"{fallback} on cpu"
            self.warm_up()

    def warm_up(self) -> None:
        noise = (np.random.default_rng(0).standard_normal(CAPTURE_RATE) * 0.001).astype(np.float32)
        segments, _ = self.model.transcribe(noise, language=self.language, beam_size=1, vad_filter=False)
        list(segments)

    def transcribe(self, audio: np.ndarray) -> str:
        if audio.size < CAPTURE_RATE * 0.3:
            return ""
        segments, _ = self.model.transcribe(
            audio,
            language=self.language,
            beam_size=self.beam_size,
            vad_filter=True,
            initial_prompt=self.initial_prompt,
            condition_on_previous_text=False,
        )
        text = " ".join(segment.text.strip() for segment in segments).strip()
        if not text or HALLUCINATION_PATTERN.search(text):
            return ""
        if self.initial_prompt and text.strip(" .").lower() in self.initial_prompt.lower():
            return ""
        return text


class Recorder:
    def __init__(self, device):
        import sounddevice

        self.lock = threading.Lock()
        self.chunks: list[np.ndarray] = []
        self.active = False
        self.stream = sounddevice.InputStream(
            samplerate=CAPTURE_RATE,
            channels=1,
            dtype="float32",
            device=device,
            callback=self._capture,
        )
        self.stream.start()

    def _capture(self, data, frames, time_info, status) -> None:
        with self.lock:
            if self.active:
                self.chunks.append(data[:, 0].copy())

    def start(self) -> None:
        with self.lock:
            self.chunks = []
            self.active = True

    def stop(self) -> np.ndarray:
        with self.lock:
            self.active = False
            chunks = self.chunks
            self.chunks = []
        if not chunks:
            return np.zeros(0, dtype=np.float32)
        return np.concatenate(chunks).astype(np.float32)

    def close(self) -> None:
        self.stream.stop()
        self.stream.close()


class AudioPlayer:
    def __init__(self, device, volume: float):
        import sounddevice

        self.volume = float(volume)
        self.generation = 0
        self.lock = threading.Lock()
        self.items: queue.Queue = queue.Queue()
        self.stream = sounddevice.OutputStream(
            samplerate=PLAYBACK_RATE,
            channels=1,
            dtype="float32",
            device=device,
            blocksize=PLAYBACK_BLOCK,
        )
        self.stream.start()
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def current_generation(self) -> int:
        with self.lock:
            return self.generation

    def stop(self) -> int:
        with self.lock:
            self.generation += 1
            generation = self.generation
        while True:
            try:
                self.items.get_nowait()
            except queue.Empty:
                break
        return generation

    def play(self, samples: np.ndarray, sample_rate: int, generation: int) -> None:
        audio = resample(samples, sample_rate, PLAYBACK_RATE) * self.volume
        self.items.put((generation, np.clip(audio, -1.0, 1.0).astype(np.float32)))

    def play_tone(self, name: str, generation: int | None = None) -> None:
        self.play(TONES[name], PLAYBACK_RATE, self.current_generation() if generation is None else generation)

    def _run(self) -> None:
        while True:
            item = self.items.get()
            if item is None:
                return
            generation, audio = item
            for start in range(0, len(audio), PLAYBACK_BLOCK):
                if generation != self.generation:
                    break
                self.stream.write(audio[start:start + PLAYBACK_BLOCK].reshape(-1, 1))

    def close(self) -> None:
        self.stop()
        self.items.put(None)
        self.thread.join(timeout=2)
        self.stream.stop()
        self.stream.close()


class PiperSpeaker:
    def __init__(self, settings: dict, base_dir: Path):
        from piper import PiperVoice, SynthesisConfig

        model_path = (base_dir / settings.get("piper_model", "voices/fr_FR-siwis-medium.onnx")).resolve()
        self.ensure_voice(model_path)
        self.voice = PiperVoice.load(str(model_path), use_cuda=bool(settings.get("piper_use_cuda", False)))
        self.config = SynthesisConfig(length_scale=float(settings.get("piper_length_scale", 1.0)))

    @staticmethod
    def ensure_voice(model_path: Path) -> None:
        config_path = model_path.with_name(model_path.name + ".json")
        if model_path.is_file() and model_path.stat().st_size > 0 and config_path.is_file() and config_path.stat().st_size > 0:
            return
        from piper.download_voices import download_voice

        voice_name = model_path.name.removesuffix(".onnx")
        temporary_dir = model_path.parent / ".download"
        print(f"Downloading voice {voice_name} (about 60 MB, first launch only)...", flush=True)
        try:
            shutil.rmtree(temporary_dir, ignore_errors=True)
            temporary_dir.mkdir(parents=True, exist_ok=True)
            download_voice(voice_name, temporary_dir, force_redownload=True)
            for downloaded in (temporary_dir / model_path.name, temporary_dir / config_path.name):
                os.replace(downloaded, model_path.parent / downloaded.name)
        except Exception as error:
            raise RuntimeError(
                f"Could not download the Piper voice {voice_name}: {error}\n"
                f"Download it manually with: python -m piper.download_voices {voice_name} --download-dir \"{model_path.parent}\"\n"
                "Or use the online voice instead by setting engine = \"edge\" in the [tts] section of coach.toml."
            ) from error
        finally:
            shutil.rmtree(temporary_dir, ignore_errors=True)
        print("Voice downloaded.", flush=True)

    def synthesize(self, text: str) -> tuple[np.ndarray, int]:
        chunks = []
        sample_rate = 22050
        for chunk in self.voice.synthesize(text, syn_config=self.config):
            sample_rate = chunk.sample_rate
            chunks.append(np.frombuffer(chunk.audio_int16_bytes, dtype=np.int16))
        if not chunks:
            return np.zeros(0, dtype=np.float32), sample_rate
        return np.concatenate(chunks).astype(np.float32) / 32768.0, sample_rate


class EdgeSpeaker:
    def __init__(self, settings: dict, base_dir: Path):
        import edge_tts
        import miniaudio

        self.edge_tts = edge_tts
        self.miniaudio = miniaudio
        self.voice = settings.get("edge_voice", "fr-FR-HenriNeural")
        self.rate = settings.get("edge_rate", "+0%")

    async def _fetch(self, text: str) -> bytes:
        communicate = self.edge_tts.Communicate(text, self.voice, rate=self.rate)
        buffer = bytearray()
        async for chunk in communicate.stream():
            if chunk.get("type") == "audio":
                buffer.extend(chunk["data"])
        return bytes(buffer)

    def synthesize(self, text: str) -> tuple[np.ndarray, int]:
        data = asyncio.run(self._fetch(text))
        if not data:
            return np.zeros(0, dtype=np.float32), 24000
        decoded = self.miniaudio.decode(data, output_format=self.miniaudio.SampleFormat.SIGNED16, nchannels=1, sample_rate=24000)
        samples = np.array(decoded.samples, dtype=np.int16).astype(np.float32) / 32768.0
        return samples, decoded.sample_rate


def create_speaker(settings: dict, base_dir: Path):
    engine = str(settings.get("engine", "piper")).lower()
    if engine == "piper":
        return PiperSpeaker(settings, base_dir)
    if engine == "edge":
        return EdgeSpeaker(settings, base_dir)
    raise ValueError(f"Unknown TTS engine '{engine}', expected 'piper' or 'edge'")


@dataclass(eq=False)
class Job:
    job_id: int
    audio: np.ndarray
    screenshot: Path | None
    generation: int
    screenshot_forced: bool = True
    released_at: float = 0.0
    first_audio_at: float | None = None
    cancel_event: threading.Event = field(default_factory=threading.Event)
    first_audio_event: threading.Event = field(default_factory=threading.Event)


class SpeechWorker:
    def __init__(self, speaker, player: AudioPlayer):
        self.speaker = speaker
        self.player = player
        self.items: queue.Queue = queue.Queue()
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def say(self, job: Job, text: str) -> None:
        self.items.put((job, text))

    def close(self) -> None:
        self.items.put(None)

    def _run(self) -> None:
        while True:
            item = self.items.get()
            if item is None:
                return
            job, text = item
            if job.cancel_event.is_set():
                continue
            try:
                audio, sample_rate = self.speaker.synthesize(text)
            except Exception:
                logger.exception("Speech synthesis failed for: %s", text)
                continue
            if job.cancel_event.is_set() or audio.size == 0:
                continue
            self.player.play(audio, sample_rate, job.generation)
            if job.first_audio_at is None:
                job.first_audio_at = time.monotonic()
                job.first_audio_event.set()


def normalize_text(text: str) -> str:
    decomposed = unicodedata.normalize("NFKD", text.lower().replace("’", "'"))
    return "".join(character for character in decomposed if not unicodedata.combining(character))


class ScreenKeywordMatcher:
    def __init__(self, keywords) -> None:
        cleaned = [normalize_text(str(keyword)).strip() for keyword in keywords]
        cleaned = [keyword for keyword in cleaned if keyword]
        self.pattern = re.compile(r"(?<!\w)(?:" + "|".join(re.escape(keyword) for keyword in cleaned) + r")(?!\w)") if cleaned else None

    def matches(self, question: str) -> bool:
        return bool(self.pattern and self.pattern.search(normalize_text(question)))


def foreground_window_rect() -> tuple[int, int, int, int] | None:
    if not IS_WINDOWS:
        return None
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    handle = user32.GetForegroundWindow()
    if not handle:
        return None
    rect = wintypes.RECT()
    if not user32.GetWindowRect(handle, ctypes.byref(rect)):
        return None
    return rect.left, rect.top, rect.right, rect.bottom


def pick_monitor(monitors: list[dict], setting) -> dict:
    physical = monitors[1:] or monitors
    if isinstance(setting, int) and not isinstance(setting, bool) and 0 <= setting < len(monitors):
        return monitors[setting]
    window = foreground_window_rect()
    if window is None:
        return physical[0]
    left, top, right, bottom = window

    def overlap(monitor: dict) -> int:
        width = min(right, monitor["left"] + monitor["width"]) - max(left, monitor["left"])
        height = min(bottom, monitor["top"] + monitor["height"]) - max(top, monitor["top"])
        return max(width, 0) * max(height, 0)

    return max(physical, key=overlap)


class ScreenshotTaker:
    def __init__(self, settings: dict, directory: Path):
        monitor = settings.get("monitor", "auto")
        self.monitor = monitor if isinstance(monitor, int) else str(monitor).lower()
        self.max_width = int(settings.get("max_width", 1600))
        self.quality = int(settings.get("jpeg_quality", 80))
        self.keep = int(settings.get("keep_last", 5))
        self.directory = directory
        self.directory.mkdir(parents=True, exist_ok=True)

    def capture(self, job_id: int) -> Path | None:
        import mss
        from PIL import Image

        grabber_factory = getattr(mss, "MSS", None) or mss.mss
        try:
            with grabber_factory() as grabber:
                raw = grabber.grab(pick_monitor(grabber.monitors, self.monitor))
                image = Image.frombytes("RGB", raw.size, raw.bgra, "raw", "BGRX")
            if image.width > self.max_width:
                height = round(image.height * self.max_width / image.width)
                image = image.resize((self.max_width, height), Image.LANCZOS)
            target = self.directory / f"screen_{job_id:05d}_{int(time.time() * 1000) % 100000:05d}.jpg"
            image.save(target, "JPEG", quality=self.quality)
            self._prune()
            return target
        except Exception:
            logger.exception("Screenshot failed")
            return None

    def _prune(self) -> None:
        files = sorted(self.directory.glob("screen_*.jpg"), key=lambda path: path.stat().st_mtime)
        for path in files[: max(len(files) - self.keep, 0)]:
            self.discard(path)

    @staticmethod
    def discard(path: Path | None) -> None:
        if path is None:
            return
        try:
            path.unlink()
        except OSError:
            pass


@dataclass(frozen=True)
class Binding:
    device: str
    name: str

    def label(self) -> str:
        return f"{self.device}:{self.name}"


def parse_binding(value: str | None) -> Binding | None:
    value = (value or "").strip().lower()
    if not value:
        return None
    device, separator, name = value.partition(":")
    if not separator or device not in ("key", "mouse") or not name.strip():
        raise ValueError(f"Invalid binding '{value}', expected key:<name> or mouse:<button>")
    return Binding(device, name.strip())


def key_aliases(key) -> set[str]:
    from pynput import keyboard

    aliases = set()
    code = key
    if isinstance(key, keyboard.Key):
        aliases.add(key.name)
        code = key.value
    char = getattr(code, "char", None)
    if char and char.isprintable():
        aliases.add(char.lower())
    vk = getattr(code, "vk", None)
    if vk is not None:
        aliases.add(f"vk{vk}")
    return aliases


class InputListener:
    def __init__(self, bindings: dict[str, Binding | None], on_press: Callable[[str], None], on_release: Callable[[str], None]):
        self.bindings = {action: binding for action, binding in bindings.items() if binding}
        self.on_press = on_press
        self.on_release = on_release
        self.held: set[str] = set()
        self.listeners = []

    def start(self) -> None:
        from pynput import keyboard, mouse

        if any(binding.device == "key" for binding in self.bindings.values()):
            listener = keyboard.Listener(on_press=self._key_down, on_release=self._key_up)
            listener.start()
            self.listeners.append(listener)
        if any(binding.device == "mouse" for binding in self.bindings.values()):
            listener = mouse.Listener(on_click=self._click)
            listener.start()
            self.listeners.append(listener)

    def stop(self) -> None:
        for listener in self.listeners:
            listener.stop()

    def _key_down(self, key, *extra) -> None:
        if key is not None:
            self._dispatch("key", key_aliases(key), True)

    def _key_up(self, key, *extra) -> None:
        if key is not None:
            self._dispatch("key", key_aliases(key), False)

    def _click(self, x, y, button, pressed, *extra) -> None:
        self._dispatch("mouse", {button.name}, pressed)

    def _dispatch(self, device: str, names: set[str], pressed: bool) -> None:
        for action, binding in self.bindings.items():
            if binding.device != device or binding.name not in names:
                continue
            if pressed:
                if action in self.held:
                    continue
                self.held.add(action)
                callback = self.on_press
            else:
                if action not in self.held:
                    continue
                self.held.discard(action)
                callback = self.on_release
            try:
                callback(action)
            except Exception:
                logger.exception("Input handler failed for %s", action)


def device_setting(value):
    if value in ("", None):
        return None
    return value


def format_timing(released_at: float, transcribed_at: float | None, first_audio_at: float | None, done_at: float) -> str:
    parts = []
    if transcribed_at is not None:
        parts.append(f"transcription {transcribed_at - released_at:.1f} s")
    if first_audio_at is not None:
        parts.append(f"first audio {first_audio_at - released_at:.1f} s")
    parts.append(f"full answer {done_at - released_at:.1f} s")
    return " | ".join(parts)


class Coach:
    def __init__(self, config: dict, base_dir: Path):
        general = config.get("general", {})
        bindings_config = config.get("bindings", {})
        audio_config = config.get("audio", {})
        self.language = general.get("language", "fr")
        self.messages = MESSAGES.get(self.language, MESSAGES["en"])
        self.interrupt_on_talk = bool(bindings_config.get("interrupt_on_push_to_talk", True))
        self.release_tail = float(bindings_config.get("release_tail_ms", 200)) / 1000
        self.min_seconds = float(bindings_config.get("min_recording_seconds", 0.35))
        self.beeps = bool(audio_config.get("beeps", True))
        self.bindings = {action: parse_binding(bindings_config.get(action)) for action in ACTIONS}
        print("Starting backend...", flush=True)
        self.backend = create_backend(config, base_dir)
        self.backend.warm_up()
        print("Loading speech recognition...", flush=True)
        self.transcriber = Transcriber(config.get("stt", {}), self.language)
        print(f"Speech recognition: {self.transcriber.description}", flush=True)
        print("Loading voice...", flush=True)
        self.player = AudioPlayer(device_setting(audio_config.get("output_device")), audio_config.get("output_volume", 1.0))
        self.speech = SpeechWorker(create_speaker(config.get("tts", {}), base_dir), self.player)
        self.recorder = Recorder(device_setting(audio_config.get("input_device")))
        screenshot_config = config.get("screenshot", {})
        self.screens = ScreenshotTaker(screenshot_config, self.backend.work_dir / "screens")
        self.auto_screenshot = bool(screenshot_config.get("auto_on_keywords", True))
        self.screen_keywords = ScreenKeywordMatcher(screenshot_config.get("keywords", DEFAULT_SCREEN_KEYWORDS))
        self.armed_ttl = float(screenshot_config.get("armed_seconds", 90))
        self.armed_screenshot: tuple[Path, float] | None = None
        self.listener = InputListener(self.bindings, self.on_press, self.on_release)
        self.jobs: queue.Queue = queue.Queue()
        self.active_jobs: set[Job] = set()
        self.state_lock = threading.Lock()
        self.job_ids = itertools.count(1)
        self.recording_action: str | None = None
        self.pending_job_id = 0
        self.pending_released_at = 0.0
        self.pending_screenshot: dict | None = None
        self.shutdown_event = threading.Event()
        self.worker = threading.Thread(target=self._process_jobs, daemon=True)

    def run(self) -> None:
        self.worker.start()
        self.listener.start()
        print(f"\nBackend: {self.backend.name}")
        session_id = getattr(self.backend, "session_id", None)
        if session_id:
            print(f"Resuming session {session_id}")
        for action, binding in self.bindings.items():
            print(f"  {action:<30} {binding.label() if binding else '(disabled)'}")
        if self.auto_screenshot:
            print("  auto screenshot when the question mentions the screen: on")
        print("Ctrl+C to quit.\n", flush=True)
        self._say_system(self.messages["ready"])
        try:
            while not self.shutdown_event.wait(0.5):
                pass
        except KeyboardInterrupt:
            pass
        finally:
            self.close()

    def close(self) -> None:
        self.shutdown_event.set()
        self.listener.stop()
        self._interrupt()
        self.jobs.put(None)
        self.backend.close()
        self.speech.close()
        self.recorder.close()
        self.player.close()

    def on_press(self, action: str) -> None:
        if action in RECORDING_ACTIONS:
            self._begin_recording(action)
        elif action == "screenshot":
            threading.Thread(target=self._arm_screenshot, daemon=True).start()
        elif action == "stop":
            self._interrupt()
            self._tone("stop")
        elif action == "new_session":
            self._interrupt()
            self._tone("reset")
            self._say_system(self.messages["new_session"])
            print("--- new session ---", flush=True)
            threading.Thread(target=self.backend.reset, daemon=True).start()

    def on_release(self, action: str) -> None:
        with self.state_lock:
            if action != self.recording_action:
                return
            self.pending_released_at = time.monotonic()
        timer = threading.Timer(self.release_tail, self._finish_recording)
        timer.daemon = True
        timer.start()

    def _begin_recording(self, action: str) -> None:
        with self.state_lock:
            if self.recording_action:
                return
            self.recording_action = action
            self.pending_job_id = next(self.job_ids)
            job_id = self.pending_job_id
        if self.interrupt_on_talk:
            self._interrupt()
        self._tone("start")
        self.recorder.start()
        forced = action == "push_to_talk_with_screenshot"
        if forced or self.auto_screenshot:
            holder = {"path": None, "forced": forced, "done": threading.Event()}
            with self.state_lock:
                self.pending_screenshot = holder

            def capture() -> None:
                holder["path"] = self.screens.capture(job_id)
                holder["done"].set()

            threading.Thread(target=capture, daemon=True).start()

    def _arm_screenshot(self) -> None:
        path = self.screens.capture(next(self.job_ids))
        if path is None:
            self._tone("error")
            return
        with self.state_lock:
            previous = self.armed_screenshot
            self.armed_screenshot = (path, time.monotonic())
        if previous is not None:
            self.screens.discard(previous[0])
        print(f"[screenshot] ready for the next question: {path.name}", flush=True)
        self._tone("reset")
        self._say_system(self.messages["screenshot_armed"])

    def _take_armed_screenshot(self) -> Path | None:
        with self.state_lock:
            armed = self.armed_screenshot
            self.armed_screenshot = None
        if armed is None:
            return None
        path, taken_at = armed
        if time.monotonic() - taken_at > self.armed_ttl:
            self.screens.discard(path)
            return None
        return path

    def _finish_recording(self) -> None:
        audio = self.recorder.stop()
        with self.state_lock:
            self.recording_action = None
            job_id = self.pending_job_id
            released_at = self.pending_released_at
            holder = self.pending_screenshot
            self.pending_screenshot = None
        screenshot = None
        forced = False
        if holder is not None:
            holder["done"].wait(3)
            screenshot = holder["path"]
            forced = holder["forced"]
        if audio.size < CAPTURE_RATE * self.min_seconds:
            self.screens.discard(screenshot)
            self._tone("error")
            return
        armed = self._take_armed_screenshot()
        if armed is not None:
            self.screens.discard(screenshot)
            screenshot, forced = armed, True
        job = Job(
            job_id=job_id,
            audio=audio,
            screenshot=screenshot,
            generation=self.player.current_generation(),
            screenshot_forced=forced,
            released_at=released_at,
        )
        with self.state_lock:
            self.active_jobs.add(job)
        self._tone("stop", job.generation)
        self.jobs.put(job)

    def _interrupt(self) -> None:
        with self.state_lock:
            jobs = list(self.active_jobs)
        for job in jobs:
            job.cancel_event.set()
        self.player.stop()

    def _tone(self, name: str, generation: int | None = None) -> None:
        if self.beeps:
            self.player.play_tone(name, generation)

    def _say_system(self, text: str) -> None:
        job = Job(job_id=0, audio=np.zeros(0, dtype=np.float32), screenshot=None, generation=self.player.current_generation())
        self.speech.say(job, text)

    def _process_jobs(self) -> None:
        while True:
            job = self.jobs.get()
            if job is None:
                return
            try:
                if not job.cancel_event.is_set():
                    self._handle(job)
            except BackendError as error:
                logger.error("Backend error: %s", error)
                print(f"[backend error] {error}", flush=True)
                if not job.cancel_event.is_set():
                    self._tone("error", job.generation)
                    self.speech.say(job, self.messages["backend_error"])
            except Exception as error:
                logger.exception("Job %s failed", job.job_id)
                print(f"[error] {error}", flush=True)
                self._tone("error", job.generation)
            finally:
                with self.state_lock:
                    self.active_jobs.discard(job)

    def _handle(self, job: Job) -> None:
        question = self.transcriber.transcribe(job.audio)
        transcribed_at = time.monotonic()
        if not question:
            self.screens.discard(job.screenshot)
            print("(nothing understood)", flush=True)
            self._tone("error", job.generation)
            return
        suffix = ""
        if job.screenshot is not None and not job.screenshot_forced:
            if self.screen_keywords.matches(question):
                suffix = " [screenshot: auto]"
            else:
                self.screens.discard(job.screenshot)
                job.screenshot = None
        elif job.screenshot is not None:
            suffix = " [screenshot]"
        print(f"\n> {question}{suffix}", flush=True)
        if job.cancel_event.is_set():
            return
        splitter = SentenceSplitter()

        def on_text(text: str) -> None:
            if job.cancel_event.is_set():
                return
            for sentence in splitter.feed(text):
                self.speech.say(job, sentence)

        answer = self.backend.ask(question, job.screenshot, job.cancel_event, on_text)
        if job.cancel_event.is_set():
            print("(interrupted)", flush=True)
            return
        for sentence in splitter.flush():
            self.speech.say(job, sentence)
        done_at = time.monotonic()
        job.first_audio_event.wait(3)
        timing = format_timing(job.released_at, transcribed_at, job.first_audio_at, done_at)
        print(f"< {answer}\n  [timing] {timing}", flush=True)
        logger.info("Timing for job %s: %s", job.job_id, timing)


def run_detection() -> None:
    from pynput import keyboard, mouse

    print("Press keys or mouse buttons to see their binding names. Ctrl+C to quit.", flush=True)

    def on_key(key, *extra) -> None:
        if key is not None:
            print("  " + "  or  ".join(f"key:{alias}" for alias in sorted(key_aliases(key))), flush=True)

    def on_click(x, y, button, pressed, *extra) -> None:
        if pressed:
            print(f"  mouse:{button.name}", flush=True)

    keyboard_listener = keyboard.Listener(on_press=on_key)
    mouse_listener = mouse.Listener(on_click=on_click)
    keyboard_listener.start()
    mouse_listener.start()
    try:
        while True:
            time.sleep(0.5)
    except KeyboardInterrupt:
        pass
    finally:
        keyboard_listener.stop()
        mouse_listener.stop()


def run_text_questions(config: dict, base_dir: Path, questions: list[str], screenshot: str | None) -> None:
    backend = create_backend(config, base_dir)
    backend.warm_up()
    print(f"Backend: {backend.name}", flush=True)
    session_id = getattr(backend, "session_id", None)
    if session_id:
        print(f"Resuming session {session_id}", flush=True)
    try:
        for index, question in enumerate(questions):
            print(f"\n> {question}", flush=True)
            splitter = SentenceSplitter()
            cancel_event = threading.Event()
            started = time.monotonic()
            first_sentence_at: list[float] = []

            def on_text(text: str) -> None:
                for sentence in splitter.feed(text):
                    if not first_sentence_at:
                        first_sentence_at.append(time.monotonic())
                    print(f"  [speech] {sentence}", flush=True)

            image = Path(screenshot) if screenshot and index == 0 else None
            answer = backend.ask(question, image, cancel_event, on_text)
            for sentence in splitter.flush():
                if not first_sentence_at:
                    first_sentence_at.append(time.monotonic())
                print(f"  [speech] {sentence}", flush=True)
            done_at = time.monotonic()
            timing = format_timing(started, None, first_sentence_at[0] if first_sentence_at else None, done_at)
            print(f"< {answer}\n  [timing] {timing}", flush=True)
    finally:
        backend.close()


def run_speech_test(config: dict, base_dir: Path, text: str) -> None:
    audio_config = config.get("audio", {})
    player = AudioPlayer(device_setting(audio_config.get("output_device")), audio_config.get("output_volume", 1.0))
    speaker = create_speaker(config.get("tts", {}), base_dir)
    started = time.monotonic()
    audio, sample_rate = speaker.synthesize(text)
    print(f"Synthesized in {time.monotonic() - started:.2f} s", flush=True)
    player.play(audio, sample_rate, player.current_generation())
    time.sleep(audio.size / sample_rate + 0.5)
    player.close()


def run_screenshot_test(config: dict, base_dir: Path) -> None:
    general = config.get("general", {})
    directory = base_dir / general.get("runtime_dir", "runtime") / "screen_test"
    taker = ScreenshotTaker({**config.get("screenshot", {}), "keep_last": 3}, directory)
    print("Switch to the game window, capture in 3 seconds...", flush=True)
    time.sleep(3)
    path = taker.capture(0)
    if path is None:
        raise RuntimeError("Screenshot failed, see coach.log")
    from PIL import Image

    with Image.open(path) as image:
        width, height = image.size
        darkest, brightest = image.convert("L").getextrema()
    print(f"Saved {path} ({width}x{height})", flush=True)
    if brightest < 16:
        print("The image is black: switch the game to windowed fullscreen.", flush=True)


def configure_logging(base_dir: Path, verbose: bool) -> None:
    file_handler = logging.FileHandler(base_dir / "coach.log", encoding="utf-8")
    file_handler.setLevel(logging.DEBUG)
    console_handler = logging.StreamHandler()
    console_handler.setLevel(logging.DEBUG if verbose else logging.WARNING)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        handlers=[file_handler, console_handler],
    )
    logger.setLevel(logging.DEBUG)


def load_config(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def main() -> None:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description="Push-to-talk voice coach for games, powered by the Claude Code or Codex CLI")
    parser.add_argument("--config", default=str(APP_DIR / "coach.toml"))
    parser.add_argument("--detect", action="store_true", help="print the binding name of each key or mouse button you press")
    parser.add_argument("--ask", action="append", help="send a text question to the backend without audio, repeat for follow-ups")
    parser.add_argument("--image", help="screenshot to attach to the first --ask question")
    parser.add_argument("--say", help="speak a sentence with the configured voice")
    parser.add_argument("--screenshot", action="store_true", help="take a test screenshot after 3 seconds and print where it was saved")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()
    if args.detect:
        run_detection()
        return
    config_path = Path(args.config).resolve()
    base_dir = config_path.parent
    configure_logging(base_dir, args.verbose)
    config = load_config(config_path)
    try:
        if args.ask:
            run_text_questions(config, base_dir, args.ask, args.image)
        elif args.say:
            run_speech_test(config, base_dir, args.say)
        elif args.screenshot:
            run_screenshot_test(config, base_dir)
        else:
            Coach(config, base_dir).run()
    except (RuntimeError, ValueError, BackendError) as error:
        print(f"\nError: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
