//! Microphone capture and speaker playback with cpal.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const CAPTURE_RATE: u32 = 16_000;

pub fn resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.len() < 2 {
        return samples.to_vec();
    }
    let target_length = ((samples.len() as f64) * f64::from(target_rate) / f64::from(source_rate)).round() as usize;
    let step = (samples.len() - 1) as f64 / (target_length.max(2) - 1) as f64;
    (0..target_length)
        .map(|index| {
            let position = index as f64 * step;
            let left = position.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (position - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

pub fn tone(frequencies: &[f32], duration: f32, volume: f32, rate: u32) -> Vec<f32> {
    let mut output = Vec::new();
    for frequency in frequencies {
        let count = (rate as f32 * duration) as usize;
        let ramp = (count / 4).min((rate as f32 * 0.006) as usize).max(1);
        for index in 0..count {
            let envelope = if index < ramp {
                index as f32 / ramp as f32
            } else if index >= count - ramp {
                (count - index) as f32 / ramp as f32
            } else {
                1.0
            };
            output.push((2.0 * std::f32::consts::PI * frequency * index as f32 / rate as f32).sin() * volume * envelope);
        }
        output.extend(std::iter::repeat_n(0.0, (rate as f32 * 0.015) as usize));
    }
    output
}

fn find_device(input: bool, name: &str) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if !name.is_empty() {
        let devices = if input { host.input_devices()? } else { host.output_devices()? };
        for device in devices {
            if device.name().map(|device_name| device_name == name).unwrap_or(false) {
                return Ok(device);
            }
        }
        log::warn!("Audio device '{name}' not found, using the default one");
    }
    let device = if input { host.default_input_device() } else { host.default_output_device() };
    device.ok_or_else(|| anyhow!(if input { "aucun micro détecté" } else { "aucune sortie audio détectée" }))
}

pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |devices: Option<Vec<cpal::Device>>| -> Vec<String> {
        devices.unwrap_or_default().iter().filter_map(|device| device.name().ok()).collect()
    };
    (
        names(host.input_devices().ok().map(Iterator::collect)),
        names(host.output_devices().ok().map(Iterator::collect)),
    )
}

/// Keeps a cpal stream alive on its own thread (streams are not Send on every platform).
struct StreamHolder {
    stop: Sender<()>,
}

impl Drop for StreamHolder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn spawn_stream<F>(build: F) -> Result<StreamHolder>
where
    F: FnOnce() -> Result<cpal::Stream> + Send + 'static,
{
    let (ready_sender, ready_receiver) = bounded::<Result<()>>(1);
    let (stop_sender, stop_receiver) = bounded::<()>(1);
    std::thread::Builder::new().name("audio-stream".into()).spawn(move || match build() {
        Ok(stream) => {
            if let Err(error) = stream.play() {
                let _ = ready_sender.send(Err(anyhow!("cannot start audio stream: {error}")));
                return;
            }
            let _ = ready_sender.send(Ok(()));
            let _ = stop_receiver.recv();
            drop(stream);
        }
        Err(error) => {
            let _ = ready_sender.send(Err(error));
        }
    })?;
    ready_receiver.recv().map_err(|_| anyhow!("audio thread stopped"))??;
    Ok(StreamHolder { stop: stop_sender })
}

#[derive(Default)]
struct RecorderShared {
    active: bool,
    samples: Vec<f32>,
    /// Loudness (RMS) of the latest captured block, for the overlay animation.
    level: f32,
}

pub struct Recorder {
    shared: Arc<Mutex<RecorderShared>>,
    rate: u32,
    _stream: StreamHolder,
}

impl Recorder {
    pub fn open(device_name: &str) -> Result<Self> {
        let device = find_device(true, device_name)?;
        let supported = device.default_input_config().context("micro: configuration introuvable")?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let channels = config.channels as usize;
        let rate = config.sample_rate.0;
        let shared = Arc::new(Mutex::new(RecorderShared::default()));
        let sink = shared.clone();
        let stream = spawn_stream(move || {
            let error_callback = |error| log::error!("Microphone stream error: {error}");
            let push = move |mono: &mut dyn Iterator<Item = f32>| {
                let mut shared = sink.lock();
                if shared.active {
                    let start = shared.samples.len();
                    shared.samples.extend(mono);
                    let block = &shared.samples[start..];
                    if !block.is_empty() {
                        let rms = (block.iter().map(|sample| sample * sample).sum::<f32>() / block.len() as f32).sqrt();
                        shared.level = rms;
                    }
                }
            };
            let stream = match format {
                SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        push(&mut data.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32))
                    },
                    error_callback,
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        push(&mut data.chunks(channels).map(|frame| {
                            frame.iter().map(|sample| f32::from(*sample) / 32768.0).sum::<f32>() / channels as f32
                        }))
                    },
                    error_callback,
                    None,
                ),
                SampleFormat::I32 => device.build_input_stream(
                    &config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        push(&mut data.chunks(channels).map(|frame| {
                            frame.iter().map(|sample| *sample as f32 / 2_147_483_648.0).sum::<f32>() / channels as f32
                        }))
                    },
                    error_callback,
                    None,
                ),
                other => return Err(anyhow!("format micro non géré: {other:?}")),
            };
            stream.map_err(|error| anyhow!("micro: {error}"))
        })?;
        Ok(Self { shared, rate, _stream: stream })
    }

    pub fn start(&self) {
        let mut shared = self.shared.lock();
        shared.samples.clear();
        shared.active = true;
    }

    /// Stops recording and returns 16 kHz mono audio.
    pub fn stop(&self) -> Vec<f32> {
        let samples = {
            let mut shared = self.shared.lock();
            shared.active = false;
            shared.level = 0.0;
            std::mem::take(&mut shared.samples)
        };
        resample(&samples, self.rate, CAPTURE_RATE)
    }

    pub fn level(&self) -> f32 {
        self.shared.lock().level
    }
}

struct PlayerShared {
    queue: VecDeque<f32>,
    volume: f32,
}

pub struct Player {
    shared: Arc<Mutex<PlayerShared>>,
    generation: AtomicU64,
    rate: Mutex<u32>,
    stream: Mutex<Option<StreamHolder>>,
}

impl Player {
    pub fn new(device_name: &str, volume: f32) -> Result<Self> {
        let player = Self {
            shared: Arc::new(Mutex::new(PlayerShared { queue: VecDeque::new(), volume })),
            generation: AtomicU64::new(0),
            rate: Mutex::new(48_000),
            stream: Mutex::new(None),
        };
        player.open(device_name)?;
        Ok(player)
    }

    pub fn open(&self, device_name: &str) -> Result<()> {
        self.stream.lock().take();
        let device = find_device(false, device_name)?;
        let supported = device.default_output_config().context("sortie audio: configuration introuvable")?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let channels = config.channels as usize;
        *self.rate.lock() = config.sample_rate.0;
        let source = self.shared.clone();
        let holder = spawn_stream(move || {
            let error_callback = |error| log::error!("Speaker stream error: {error}");
            // The queue is locked once per audio buffer, not once per sample.
            fn next(shared: &mut PlayerShared) -> f32 {
                let volume = shared.volume;
                shared.queue.pop_front().map(|sample| (sample * volume).clamp(-1.0, 1.0)).unwrap_or(0.0)
            }
            let stream = match format {
                SampleFormat::F32 => device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut shared = source.lock();
                        for frame in data.chunks_mut(channels) {
                            frame.fill(next(&mut shared));
                        }
                    },
                    error_callback,
                    None,
                ),
                SampleFormat::I16 => device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let mut shared = source.lock();
                        for frame in data.chunks_mut(channels) {
                            frame.fill((next(&mut shared) * 32767.0) as i16);
                        }
                    },
                    error_callback,
                    None,
                ),
                other => return Err(anyhow!("format de sortie non géré: {other:?}")),
            };
            stream.map_err(|error| anyhow!("sortie audio: {error}"))
        })?;
        *self.stream.lock() = Some(holder);
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.lock().volume = volume;
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Cuts the current audio and invalidates everything queued for older generations.
    pub fn stop(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.lock().queue.clear();
        generation
    }

    pub fn play(&self, samples: &[f32], sample_rate: u32, generation: u64) {
        if generation != self.generation() {
            return;
        }
        let audio = resample(samples, sample_rate, *self.rate.lock());
        let mut shared = self.shared.lock();
        if generation == self.generation() {
            shared.queue.extend(audio);
        }
    }

    /// `volume` scales the beeps on top of the main volume (1.0 = default loudness).
    pub fn play_tone(&self, name: &str, generation: Option<u64>, volume: f32) {
        let rate = *self.rate.lock();
        let amplitude = 0.18 * volume.clamp(0.0, 3.0);
        if amplitude <= 0.0 {
            return;
        }
        let samples = match name {
            "start" => tone(&[660.0, 880.0], 0.07, amplitude, rate),
            "stop" => tone(&[880.0, 660.0], 0.07, amplitude, rate),
            "error" => tone(&[300.0, 220.0], 0.12, amplitude, rate),
            "reset" => tone(&[523.0, 659.0, 784.0], 0.07, amplitude, rate),
            _ => return,
        };
        self.play(&samples, rate, generation.unwrap_or_else(|| self.generation()));
    }

    pub fn is_busy(&self) -> bool {
        !self.shared.lock().queue.is_empty()
    }
}
