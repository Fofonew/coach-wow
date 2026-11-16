//! Natural neural voices of Microsoft Edge "Read aloud" (online, unofficial service).
//! Piper stays available offline and is used as a fallback when this fails.

use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Sender};
use msedge_tts::tts::client::{connect, MSEdgeTTSClient};
use msedge_tts::tts::SpeechConfig;
use parking_lot::Mutex;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 24_000;
/// The read-aloud service only streams MP3 or Opus.
const AUDIO_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

fn decode_mp3(bytes: Vec<u8>) -> Result<(Vec<f32>, u32)> {
    let mut decoder = minimp3::Decoder::new(std::io::Cursor::new(bytes));
    let mut samples = Vec::new();
    let mut rate = SAMPLE_RATE;
    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                rate = frame.sample_rate.max(1) as u32;
                let channels = frame.channels.max(1);
                samples.extend(
                    frame
                        .data
                        .chunks(channels)
                        .map(|chunk| chunk.iter().map(|sample| f32::from(*sample)).sum::<f32>() / (channels as f32 * 32768.0)),
                );
            }
            Err(minimp3::Error::Eof) => break,
            Err(minimp3::Error::SkippedData) => continue,
            Err(error) => return Err(anyhow!("décodage de la voix : {error:?}")),
        }
    }
    Ok((samples, rate))
}

pub struct EdgeVoice {
    pub id: &'static str,
    pub label: &'static str,
}

pub const VOICES: [EdgeVoice; 8] = [
    EdgeVoice { id: "fr-FR-VivienneMultilingualNeural", label: "Vivienne (française, très naturelle)" },
    EdgeVoice { id: "fr-FR-RemyMultilingualNeural", label: "Rémy (français, très naturel)" },
    EdgeVoice { id: "fr-FR-DeniseNeural", label: "Denise (française)" },
    EdgeVoice { id: "fr-FR-HenriNeural", label: "Henri (français)" },
    EdgeVoice { id: "fr-CA-SylvieNeural", label: "Sylvie (québécoise)" },
    EdgeVoice { id: "fr-CA-AntoineNeural", label: "Antoine (québécois)" },
    EdgeVoice { id: "en-US-AvaMultilingualNeural", label: "Ava (anglais)" },
    EdgeVoice { id: "en-US-AndrewMultilingualNeural", label: "Andrew (anglais)" },
];

pub fn is_edge_voice(id: &str) -> bool {
    VOICES.iter().any(|voice| voice.id == id)
}

/// Both ring and aws-lc-rs end up linked: rustls needs to be told which one to use.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

type Client = MSEdgeTTSClient<TcpStream>;
type Audio = (Vec<f32>, u32);

/// Connections kept open to the service, so that several sentences are prepared at once.
const CONNECTIONS: usize = 4;
/// The service sometimes takes many seconds for one sentence. A request that has not
/// answered after this long is asked again on another connection, and the first answer wins.
const ASK_AGAIN_AFTER: Duration = Duration::from_millis(1500);
/// Past this, the sentence is left to the offline voice.
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

struct Shared {
    voice: String,
    rate: i32,
    clients: [Mutex<Option<Client>>; CONNECTIONS],
}

impl Shared {
    /// One request on a free connection, or on a temporary one when all are busy: a
    /// sentence never waits behind a slow one. `text` is already escaped.
    fn request(&self, text: &str) -> Result<Audio> {
        let config = SpeechConfig { voice_name: self.voice.clone(), audio_format: AUDIO_FORMAT.into(), pitch: 0, rate: self.rate, volume: 0 };
        let mut slot = self.clients.iter().find_map(|slot| slot.try_lock());
        let mut temporary = None;
        let client: &mut Option<Client> = match slot.as_mut() {
            Some(guard) => guard,
            None => &mut temporary,
        };
        // The service closes idle connections: retry once on a fresh one.
        for attempt in 0..2 {
            if client.is_none() {
                *client = Some(connect().map_err(|error| anyhow!("voix en ligne injoignable : {error}"))?);
            }
            let result = client.as_mut().map(|connection| connection.synthesize(text, &config));
            match result {
                Some(Ok(audio)) if !audio.audio_bytes.is_empty() => return decode_mp3(audio.audio_bytes),
                Some(Ok(_)) if attempt == 1 => return Err(anyhow!("la voix en ligne n'a renvoyé aucun son")),
                Some(Err(error)) if attempt == 1 => return Err(anyhow!("voix en ligne : {error}")),
                _ => *client = None,
            }
        }
        Err(anyhow!("voix en ligne indisponible"))
    }
}

pub struct EdgeSpeaker {
    shared: Arc<Shared>,
}

impl EdgeSpeaker {
    /// `speed` is the same multiplier as for Piper (1.0 = normal).
    pub fn new(voice: &str, speed: f32) -> Result<Self> {
        ensure_crypto_provider();
        let speaker = Self {
            shared: Arc::new(Shared {
                voice: voice.to_string(),
                rate: ((speed.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32,
                clients: std::array::from_fn(|_| Mutex::new(None)),
            }),
        };
        speaker.synthesize("Bonjour.")?;
        Ok(speaker)
    }

    fn launch(&self, text: &str, answers: &Sender<Result<Audio>>) {
        let (shared, text, answers) = (self.shared.clone(), text.to_string(), answers.clone());
        std::thread::spawn(move || {
            let _ = answers.send(shared.request(&text));
        });
    }

    pub fn synthesize(&self, text: &str) -> Result<Audio> {
        let text = escape_xml(text);
        let (answers, results) = unbounded();
        let started = Instant::now();
        self.launch(&text, &answers);
        let mut running = 1;
        let mut asked_again = false;
        let mut failure = None;
        loop {
            let wait = if asked_again { GIVE_UP_AFTER.saturating_sub(started.elapsed()) } else { ASK_AGAIN_AFTER };
            match results.recv_timeout(wait) {
                Ok(Ok(audio)) => return Ok(audio),
                Ok(Err(error)) => {
                    running -= 1;
                    failure = Some(error);
                    if running > 0 {
                        continue;
                    }
                    if asked_again {
                        break;
                    }
                }
                Err(_) if asked_again => break,
                Err(_) => log::debug!("Voice: no answer after {:.1} s, asking again on another connection", started.elapsed().as_secs_f32()),
            }
            asked_again = true;
            self.launch(&text, &answers);
            running += 1;
        }
        Err(failure.unwrap_or_else(|| anyhow!("la voix en ligne ne répond pas")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs internet access; skipped silently when the service can't be reached.
    #[test]
    fn synthesizes_french_pcm() {
        let speaker = match EdgeSpeaker::new("fr-FR-VivienneMultilingualNeural", 1.0) {
            Ok(speaker) => speaker,
            Err(error) => {
                println!("edge voice unavailable: {error:#}");
                return;
            }
        };
        let started = std::time::Instant::now();
        let (samples, rate) = speaker.synthesize("Je t'ai mis la macro dans le chat, glisse-la sur ta barre & c'est bon <3").unwrap();
        let seconds = samples.len() as f32 / rate as f32;
        println!("edge voice: {seconds:.2} s of audio in {:.2} s", started.elapsed().as_secs_f32());
        assert!(seconds > 1.0, "audio too short: {seconds}");
        assert!(samples.iter().any(|sample| sample.abs() > 0.05), "silent audio");

        // Several sentences at once, each on its own connection.
        let started = std::time::Instant::now();
        let sentences = ["Je vérifie ça tout de suite.", "Tu peux vendre la baguette au marchand.", "Glisse-la sur ta barre d'action."];
        let seconds: Vec<f32> = std::thread::scope(|scope| {
            let handles: Vec<_> = sentences.iter().map(|sentence| scope.spawn(|| speaker.synthesize(sentence))).collect();
            handles
                .into_iter()
                .map(|handle| match handle.join().unwrap() {
                    Ok((samples, rate)) => samples.len() as f32 / rate as f32,
                    Err(error) => {
                        println!("edge voice: parallel sentence failed: {error:#}");
                        0.0
                    }
                })
                .collect()
        });
        println!("edge voice: {seconds:?} s of audio for 3 sentences at once in {:.2} s", started.elapsed().as_secs_f32());
        assert!(seconds.iter().all(|length| *length > 0.8), "parallel synthesis failed: {seconds:?}");
    }
}
