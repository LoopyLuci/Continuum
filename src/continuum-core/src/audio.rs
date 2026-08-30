use crate::*;
///
/// Implement this to add support for new audio codecs
/// (Opus, AAC, etc.) and processing (echo cancellation, noise suppression).
pub trait AudioProcessor: Send + Sync {
    /// Capture an audio frame.
    fn capture(&mut self) -> ContinuumResult<AudioFrame>;

    /// Play an audio frame.
    fn play(&mut self, frame: &AudioFrame);

    /// Check if audio capture/playback is active.
    fn is_active(&self) -> bool;

    /// Get the processor name.
    fn name(&self) -> &str;
}

/// An audio frame.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: i64,
}

/// Audio codec trait for encoding/decoding audio.
pub trait AudioCodec: Send + Sync {
    /// Encode audio samples.
    fn encode(&mut self, frame: &AudioFrame) -> ContinuumResult<Vec<u8>>;

    /// Decode audio samples.
    fn decode(&mut self, data: &[u8]) -> ContinuumResult<AudioFrame>;

    /// Get the codec name.
    fn name(&self) -> &str;
}
