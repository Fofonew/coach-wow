//! Piper voices through sherpa-onnx.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use sherpa_rs::tts::{CommonTtsConfig, VitsTts, VitsTtsConfig};
use sherpa_rs::OnnxConfig;
use std::path::Path;

/// Voices pad every clip with silence. Played one sentence after another, that padding adds
/// up to long pauses: keep a short breath after each clip and almost nothing before it.
const SILENCE_LEVEL: f32 = 0.008;
const PAUSE_BEFORE_SECONDS: f32 = 0.02;
const PAUSE_AFTER_SECONDS: f32 = 0.3;

pub fn trim_silence(samples: &mut Vec<f32>, rate: u32) {
    let Some(first) = samples.iter().position(|sample| sample.abs() > SILENCE_LEVEL) else {
        return;
    };
    let last = samples.iter().rposition(|sample| sample.abs() > SILENCE_LEVEL).unwrap_or(first);
    let end = (last + 1 + (rate as f32 * PAUSE_AFTER_SECONDS) as usize).min(samples.len());
    let start = first.saturating_sub((rate as f32 * PAUSE_BEFORE_SECONDS) as usize);
    samples.truncate(end);
    samples.drain(..start);
}

pub struct Speaker {
    tts: Option<Mutex<VitsTts>>,
    edge: Option<crate::tts_edge::EdgeSpeaker>,
    speed: f32,
}

impl Speaker {
    /// Natural online voice, with the offline Piper voice as a fallback when the service fails.
    pub fn with_edge(edge_voice: &str, piper_dir: Option<&Path>, speed: f32) -> Result<Self> {
        let fallback = piper_dir.and_then(|dir| Self::load(dir, speed).ok());
        match crate::tts_edge::EdgeSpeaker::new(edge_voice, speed) {
            Ok(edge) => Ok(Self { tts: fallback.and_then(|speaker| speaker.tts), edge: Some(edge), speed }),
            Err(error) => {
                log::warn!("Online voice unavailable ({error:#}), using the offline voice");
                fallback.ok_or(error)
            }
        }
    }

    pub fn load(voice_dir: &Path, speed: f32) -> Result<Self> {
        let model = std::fs::read_dir(voice_dir)?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "onnx"))
            .ok_or_else(|| anyhow!("voix introuvable dans {}", voice_dir.display()))?;
        let to_string = |path: &Path| path.to_string_lossy().to_string();
        let config = VitsTtsConfig {
            model: to_string(&model),
            tokens: to_string(&voice_dir.join("tokens.txt")),
            data_dir: to_string(&voice_dir.join("espeak-ng-data")),
            length_scale: 1.0,
            noise_scale: 0.667,
            noise_scale_w: 0.8,
            silence_scale: 0.2,
            onnx_config: OnnxConfig { provider: "cpu".into(), debug: false, num_threads: 2 },
            tts_config: CommonTtsConfig { max_num_sentences: 1, silence_scale: 0.2, ..Default::default() },
            ..Default::default()
        };
        let speaker = Self { tts: Some(Mutex::new(VitsTts::new(config))), edge: None, speed: speed.clamp(0.5, 2.0) };
        speaker.synthesize("Test.")?;
        Ok(speaker)
    }

    /// Audio of a sentence, without the silence the voice adds around it.
    pub fn synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        let started = std::time::Instant::now();
        let (mut samples, rate) = self.synthesize_padded(text)?;
        trim_silence(&mut samples, rate);
        log::debug!("Voice: {:.1} s of audio in {:.1} s", samples.len() as f32 / rate.max(1) as f32, started.elapsed().as_secs_f32());
        Ok((samples, rate))
    }

    fn synthesize_padded(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        if let Some(edge) = &self.edge {
            match edge.synthesize(text) {
                Ok(audio) => return Ok(audio),
                Err(error) if self.tts.is_some() => log::warn!("Online voice failed, using the offline voice: {error:#}"),
                Err(error) => return Err(error),
            }
        }
        let tts = self.tts.as_ref().ok_or_else(|| anyhow!("aucune voix disponible"))?;
        let audio = tts.lock().create(text, 0, self.speed).map_err(|error| anyhow!("synthèse vocale: {error}"))?;
        Ok((audio.samples, audio.sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_the_silence_around_a_clip() {
        let rate = 1000;
        let mut samples = vec![0.0; 150];
        samples.extend(std::iter::repeat_n(0.5, 400));
        samples.extend(std::iter::repeat_n(0.001, 500));
        trim_silence(&mut samples, rate);
        // 20 ms kept before the voice, 300 ms after it.
        assert_eq!(samples.len(), 20 + 400 + 300);
        assert!(samples[19].abs() < SILENCE_LEVEL && samples[20] == 0.5);
        let mut short = vec![0.0, 0.5, 0.5, 0.0];
        trim_silence(&mut short, rate);
        assert_eq!(short, vec![0.0, 0.5, 0.5, 0.0]);
        let mut silent = vec![0.0; 50];
        trim_silence(&mut silent, rate);
        assert_eq!(silent.len(), 50);
    }
}
