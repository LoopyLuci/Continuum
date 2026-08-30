use crate::*;

/// Video encoding and decoding trait.
///
/// Implement this to add support for new video codecs (AV1, HEVC, etc.)
/// or hardware encoders (NVENC, AMF, VAAPI, VideoToolbox).
pub trait VideoEncoder: Send + Sync {
    /// Encode a video frame.
    fn encode(
        &mut self,
        frame: &VideoFrame,
        is_keyframe: bool,
        quality: u8,
    ) -> ContinuumResult<EncodedFrame>;

    /// Notify the encoder of the current network conditions.
    fn notify_network(&mut self, rtt_ms: f32, packet_loss: f32);

    /// Get the current bitrate estimate in kbps.
    fn bitrate_estimate_kbps(&self) -> u32;

    /// Request a keyframe on the next frame.
    fn request_keyframe(&mut self);

    /// Reset the encoder state.
    fn reset(&mut self);
}

/// A video frame ready for encoding.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp_us: i64,
}

/// An encoded video frame ready for transmission.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub frame_number: u64,
    pub timestamp_us: i64,
    pub is_keyframe: bool,
    pub encode_time_us: u64,
}

/// Video decoder trait.
pub trait VideoDecoder: Send + Sync {
    /// Decode an encoded frame.
    fn decode(&mut self, frame: &EncodedFrame) -> ContinuumResult<VideoFrame>;
}
