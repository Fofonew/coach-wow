//! Speech recognition with whisper.cpp (GPU through Vulkan when available).

use crate::audio::CAPTURE_RATE;
use anyhow::{anyhow, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    context: WhisperContext,
    language: String,
    initial_prompt: String,
    threads: i32,
    pub description: String,
}

impl Transcriber {
    pub fn load(model: &Path, use_gpu: bool, language: &str, initial_prompt: &str) -> Result<Self> {
        let mut parameters = WhisperContextParameters::default();
        parameters.use_gpu = use_gpu;
        parameters.flash_attn = use_gpu;
        let path = model.to_str().ok_or_else(|| anyhow!("invalid model path"))?;
        let context = WhisperContext::new_with_params(path, parameters).map_err(|error| anyhow!("Whisper: {error}"))?;
        let threads = std::thread::available_parallelism().map(|count| count.get().min(8) as i32).unwrap_or(4);
        let name = model.file_stem().and_then(|stem| stem.to_str()).unwrap_or("whisper").trim_start_matches("ggml-").to_string();
        let transcriber = Self {
            context,
            language: language.to_string(),
            initial_prompt: initial_prompt.to_string(),
            threads,
            description: format!("{name} ({})", if use_gpu { "carte graphique" } else { "processeur" }),
        };
        transcriber.transcribe(&vec![0.0; CAPTURE_RATE as usize])?;
        Ok(transcriber)
    }

    pub fn transcribe(&self, audio: &[f32]) -> Result<String> {
        if audio.len() < (CAPTURE_RATE as f32 * 0.3) as usize {
            return Ok(String::new());
        }
        let mut padded;
        let audio = if audio.len() < CAPTURE_RATE as usize + CAPTURE_RATE as usize / 10 {
            padded = audio.to_vec();
            padded.resize(CAPTURE_RATE as usize + CAPTURE_RATE as usize / 10, 0.0);
            &padded[..]
        } else {
            audio
        };
        let mut state = self.context.create_state().map_err(|error| anyhow!("Whisper: {error}"))?;
        let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
        params.set_language(Some(&self.language));
        params.set_n_threads(self.threads);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_speech_thold(0.6);
        if !self.initial_prompt.is_empty() {
            params.set_initial_prompt(&self.initial_prompt);
        }
        state.full(params, audio).map_err(|error| anyhow!("Whisper: {error}"))?;
        let mut text = String::new();
        for segment in state.as_iter() {
            if segment.no_speech_probability() > 0.8 {
                continue;
            }
            if let Ok(piece) = segment.to_str_lossy() {
                text.push_str(piece.trim());
                text.push(' ');
            }
        }
        let text = text.trim().to_string();
        if crate::text::is_hallucination(&text, &self.initial_prompt) {
            return Ok(String::new());
        }
        Ok(text)
    }
}
