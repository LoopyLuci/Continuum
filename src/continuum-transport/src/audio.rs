use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: i64,
}

/// Holds the cpal stream alive on a dedicated thread (since cpal::Stream is !Send).
struct StreamGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub struct AudioCapturer {
    sample_rate: u32,
    channels: u16,
    frame_size: usize,
    buffer: Arc<Mutex<Vec<f32>>>,
    _guard: Option<StreamGuard>,
    is_active: bool,
}

impl AudioCapturer {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let frame_size = (sample_rate as usize / 100) * channels as usize;
        Self {
            sample_rate,
            channels,
            frame_size,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(frame_size * 4))),
            _guard: None,
            is_active: false,
        }
    }

    /// Start capturing audio from the system's default input device.
    /// Returns true if capture started successfully, false if falling back to silence.
    pub fn start(&mut self) -> bool {
        let host = cpal::default_host();

        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                tracing::warn!("No audio input device found — audio capture disabled");
                return false;
            }
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Cannot get audio input config — audio capture disabled");
                return false;
            }
        };

        let sample_rate = self.sample_rate;
        let channels = self.channels;
        let buffer = self.buffer.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let handle = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let stream = match device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut buf = buffer.lock().unwrap();
                        buf.extend_from_slice(data);
                        let max = sample_rate as usize * channels as usize * 2;
                        if buf.len() > max {
                            let drain = buf.len() - max;
                            buf.drain(..drain);
                        }
                    },
                    |err| {
                        tracing::error!(error = %err, "Audio capture stream error");
                    },
                    None,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "Cannot build audio input stream");
                        return;
                    }
                };

                if let Err(e) = stream.play() {
                    tracing::warn!(error = %e, "Cannot start audio capture");
                    return;
                }

                tracing::info!(
                    sample_rate = sample_rate,
                    channels = channels,
                    "Audio capture started"
                );
                while !stop_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });

        match handle {
            Ok(h) => {
                self._guard = Some(StreamGuard {
                    stop,
                    handle: Some(h),
                });
                self.is_active = true;
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "Cannot spawn audio capture thread");
                false
            }
        }
    }

    /// Capture one audio frame. Returns silence if no real audio is available.
    pub fn capture(&mut self) -> Result<AudioFrame> {
        let samples = {
            let mut buf = self.buffer.lock().unwrap();
            if buf.len() >= self.frame_size {
                buf.drain(..self.frame_size).collect()
            } else {
                vec![0.0f32; self.frame_size]
            }
        };

        Ok(AudioFrame {
            samples,
            sample_rate: self.sample_rate,
            channels: self.channels,
            timestamp_us: chrono::Utc::now().timestamp_micros(),
        })
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn frame_size(&self) -> usize {
        self.frame_size
    }
}

pub struct AudioPlayer {
    sample_rate: u32,
    channels: u16,
    buffer: Arc<Mutex<Vec<f32>>>,
    _guard: Option<StreamGuard>,
    volume: Arc<Mutex<f32>>,
    muted: Arc<Mutex<bool>>,
}

impl AudioPlayer {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(
                sample_rate as usize * channels as usize,
            ))),
            _guard: None,
            volume: Arc::new(Mutex::new(1.0)),
            muted: Arc::new(Mutex::new(false)),
        }
    }

    /// Start audio playback on the default output device.
    pub fn start(&mut self) -> bool {
        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => {
                tracing::warn!("No audio output device found — audio playback disabled");
                return false;
            }
        };

        let config = cpal::StreamConfig {
            channels: self.channels,
            sample_rate: cpal::SampleRate(self.sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer = self.buffer.clone();
        let volume = self.volume.clone();
        let muted = self.muted.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let handle = std::thread::Builder::new()
            .name("audio-playback".into())
            .spawn(move || {
                let stream = match device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut buf = buffer.lock().unwrap();
                        let vol = *volume.lock().unwrap();
                        let is_muted = *muted.lock().unwrap();

                        for sample in data.iter_mut() {
                            if let Some(s) = buf.first() {
                                *sample = if is_muted { 0.0 } else { s * vol };
                                buf.remove(0);
                            } else {
                                *sample = 0.0;
                            }
                        }
                    },
                    |err| {
                        tracing::error!(error = %err, "Audio playback stream error");
                    },
                    None,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "Cannot build audio playback stream");
                        return;
                    }
                };

                if let Err(e) = stream.play() {
                    tracing::warn!(error = %e, "Cannot start audio playback");
                    return;
                }

                tracing::info!("Audio playback started");
                while !stop_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });

        match handle {
            Ok(h) => {
                self._guard = Some(StreamGuard {
                    stop,
                    handle: Some(h),
                });
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "Cannot spawn audio playback thread");
                false
            }
        }
    }

    /// Queue audio samples for playback.
    pub fn play(&mut self, frame: &AudioFrame) {
        let mut buf = self.buffer.lock().unwrap();
        buf.extend_from_slice(&frame.samples);
        let max = (self.sample_rate as usize / 2) * self.channels as usize;
        if buf.len() > max {
            let drain = buf.len() - max;
            buf.drain(..drain);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        *self.volume.lock().unwrap() = volume.clamp(0.0, 2.0);
    }

    pub fn set_muted(&mut self, muted: bool) {
        *self.muted.lock().unwrap() = muted;
    }

    pub fn is_muted(&self) -> bool {
        *self.muted.lock().unwrap()
    }

    pub fn volume(&self) -> f32 {
        *self.volume.lock().unwrap()
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    pub fn buffer_ms(&self) -> f64 {
        let len = self.buffer.lock().unwrap().len();
        let samples_per_ms = (self.sample_rate as f64 / 1000.0) * self.channels as f64;
        if samples_per_ms > 0.0 {
            len as f64 / samples_per_ms
        } else {
            0.0
        }
    }

    pub fn clear(&mut self) {
        self.buffer.lock().unwrap().clear();
    }
}

/// Simple echo detection: if the capture buffer contains audio that closely
/// matches what was recently played, flag it as potential echo.
pub fn detect_echo(capture_samples: &[f32], recent_playback: &[f32]) -> bool {
    if capture_samples.len() != recent_playback.len() {
        return false;
    }
    let mut correlation = 0.0f32;
    let mut capture_energy = 0.0f32;
    let mut playback_energy = 0.0f32;
    for (c, p) in capture_samples.iter().zip(recent_playback.iter()) {
        correlation += c * p;
        capture_energy += c * c;
        playback_energy += p * p;
    }
    let denominator = (capture_energy * playback_energy).sqrt();
    if denominator < 1e-10 {
        return false;
    }
    let normalized = correlation / denominator;
    normalized > 0.7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_frame_serialization() {
        let frame = AudioFrame {
            samples: vec![0.1, 0.2, 0.3],
            sample_rate: 48000,
            channels: 2,
            timestamp_us: 12345,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let decoded: AudioFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.sample_rate, 48000);
        assert_eq!(decoded.samples.len(), 3);
    }

    #[test]
    fn test_audio_player_volume() {
        let mut player = AudioPlayer::new(48000, 2);
        assert_eq!(player.volume(), 1.0);
        player.set_volume(0.5);
        assert_eq!(player.volume(), 0.5);
        player.set_volume(3.0);
        assert_eq!(player.volume(), 2.0);
    }

    #[test]
    fn test_audio_player_mute() {
        let mut player = AudioPlayer::new(48000, 2);
        assert!(!player.is_muted());
        player.set_muted(true);
        assert!(player.is_muted());
    }

    #[test]
    fn test_audio_capturer_frame_size() {
        let capturer = AudioCapturer::new(48000, 2);
        assert_eq!(capturer.frame_size(), 960);
    }
}
