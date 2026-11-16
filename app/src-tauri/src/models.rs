//! Downloads of the speech recognition model and the Piper voice (first launch only).

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub file: &'static str,
    pub size_mb: u32,
}

pub const WHISPER_MODELS: [ModelInfo; 2] = [
    ModelInfo {
        id: "large-v3-turbo",
        label: "Précis (carte graphique conseillée)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size_mb: 548,
    },
    ModelInfo {
        id: "small",
        label: "Léger (PC modeste)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
        file: "ggml-small-q5_1.bin",
        size_mb: 182,
    },
];

pub const VOICES: [ModelInfo; 3] = [
    ModelInfo {
        id: "fr_FR-siwis-medium",
        label: "Siwis (français, femme)",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fr_FR-siwis-medium.tar.bz2",
        file: "vits-piper-fr_FR-siwis-medium",
        size_mb: 64,
    },
    ModelInfo {
        id: "fr_FR-tom-medium",
        label: "Tom (français, homme)",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-fr_FR-tom-medium.tar.bz2",
        file: "vits-piper-fr_FR-tom-medium",
        size_mb: 64,
    },
    ModelInfo {
        id: "en_US-ryan-medium",
        label: "Ryan (anglais, homme)",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-ryan-medium.tar.bz2",
        file: "vits-piper-en_US-ryan-medium",
        size_mb: 64,
    },
];

#[derive(Serialize, Clone)]
pub struct Choice {
    pub id: &'static str,
    pub label: &'static str,
    pub size_mb: u32,
}

pub fn choices(list: &[ModelInfo]) -> Vec<Choice> {
    list.iter().map(|model| Choice { id: model.id, label: model.label, size_mb: model.size_mb }).collect()
}

fn find<'a>(list: &'a [ModelInfo], id: &str) -> &'a ModelInfo {
    list.iter().find(|model| model.id == id).unwrap_or(&list[0])
}

pub fn whisper_path(models_dir: &Path, id: &str) -> PathBuf {
    models_dir.join(find(&WHISPER_MODELS, id).file)
}

pub fn voice_dir(models_dir: &Path, id: &str) -> PathBuf {
    models_dir.join("voices").join(find(&VOICES, id).file)
}

pub fn whisper_ready(models_dir: &Path, id: &str) -> bool {
    whisper_path(models_dir, id).metadata().map(|metadata| metadata.len() > 10_000_000).unwrap_or(false)
}

pub fn voice_ready(models_dir: &Path, id: &str) -> bool {
    let directory = voice_dir(models_dir, id);
    directory.join("tokens.txt").is_file() && directory.join("espeak-ng-data").is_dir()
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent("Mentor/0.1")
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(None)
        .build()?)
}

fn download(url: &str, target: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension("part");
    let mut response = client()?.get(url).send()?.error_for_status().with_context(|| format!("téléchargement de {url}"))?;
    let total = response.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&temporary)?;
    let mut buffer = vec![0u8; 1 << 16];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        done += read as u64;
        if done - last_report > 1 << 20 || done == total {
            progress(done, total);
            last_report = done;
        }
    }
    file.flush()?;
    drop(file);
    if total > 0 && done != total {
        return Err(anyhow!("téléchargement incomplet ({done} / {total} octets)"));
    }
    std::fs::rename(&temporary, target)?;
    progress(done, total.max(done));
    Ok(())
}

pub fn download_whisper(models_dir: &Path, id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let model = find(&WHISPER_MODELS, id);
    download(model.url, &models_dir.join(model.file), progress)
}

pub fn download_voice(models_dir: &Path, id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let voice = find(&VOICES, id);
    let voices_dir = models_dir.join("voices");
    let archive = voices_dir.join(format!("{}.tar.bz2", voice.file));
    download(voice.url, &archive, progress)?;
    let file = std::fs::File::open(&archive)?;
    let mut unpacker = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
    unpacker.unpack(&voices_dir).context("décompression de la voix")?;
    let _ = std::fs::remove_file(&archive);
    if !voice_ready(models_dir, id) {
        return Err(anyhow!("la voix téléchargée est incomplète"));
    }
    Ok(())
}
