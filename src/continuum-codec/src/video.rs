use image::{ImageBuffer, Rgb};
use serde::{Deserialize, Serialize};

/// A raw video frame in RGB format
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGB24 format
    pub timestamp: u64, // milliseconds
}

/// An encoded video frame
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub is_keyframe: bool,
    pub timestamp: u64,
    pub codec_type: CodecType,
}

/// Codec types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecType {
    Jpeg,
    H264,
    H265,
    Av1,
    Vp9,
}

/// Encoder configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub codec_type: CodecType,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub quality: u8, // 1-100
    pub bitrate_kbps: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            codec_type: CodecType::Jpeg,
            width: 1920,
            height: 1080,
            fps: 30,
            quality: 80,
            bitrate_kbps: 5000,
        }
    }
}

/// Decoder configuration
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    pub width: u32,
    pub height: u32,
}

/// Video encoder trait
pub trait VideoEncoder: Send {
    /// Encode a raw frame
    fn encode(&mut self, frame: &VideoFrame) -> continuum_core::ContinuumResult<EncodedFrame>;
    
    /// Encode a keyframe
    fn encode_keyframe(&mut self, frame: &VideoFrame) -> continuum_core::ContinuumResult<EncodedFrame>;
    
    /// Update bitrate
    fn set_bitrate(&mut self, kbps: u32);
    
    /// Update quality
    fn set_quality(&mut self, quality: u8);
    
    /// Get encoder capabilities
    fn capabilities(&self) -> EncoderCapabilities;
}

/// Video decoder trait
pub trait VideoDecoder: Send {
    /// Decode an encoded frame
    fn decode(&mut self, frame: &EncodedFrame) -> continuum_core::ContinuumResult<VideoFrame>;
}

/// Encoder capabilities
#[derive(Debug, Clone)]
pub struct EncoderCapabilities {
    pub codec_type: CodecType,
    pub hardware_accelerated: bool,
    pub max_resolution: (u32, u32),
    pub max_fps: u32,
}

/// Software JPEG encoder
pub struct JpegEncoder {
    config: EncoderConfig,
    quality: u8,
}

impl JpegEncoder {
    pub fn new(config: EncoderConfig) -> Self {
        Self {
            quality: config.quality,
            config,
        }
    }
}

impl VideoEncoder for JpegEncoder {
    fn encode(&mut self, frame: &VideoFrame) -> continuum_core::ContinuumResult<EncodedFrame> {
        let img = ImageBuffer::<Rgb<u8>, _>::from_raw(
            frame.width,
            frame.height,
            frame.data.clone(),
        ).ok_or_else(|| continuum_core::ContinuumError::Video("Invalid frame dimensions".into()))?;

        let mut output = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, self.quality);
        encoder.encode_image(&img)
            .map_err(|e| continuum_core::ContinuumError::Video(format!("JPEG encode error: {}", e)))?;

        Ok(EncodedFrame {
            data: output,
            width: frame.width,
            height: frame.height,
            is_keyframe: true, // JPEG is always keyframe
            timestamp: frame.timestamp,
            codec_type: CodecType::Jpeg,
        })
    }

    fn encode_keyframe(&mut self, frame: &VideoFrame) -> continuum_core::ContinuumResult<EncodedFrame> {
        self.encode(frame)
    }

    fn set_bitrate(&mut self, _kbps: u32) {
        // JPEG doesn't support bitrate control
    }

    fn set_quality(&mut self, quality: u8) {
        self.quality = quality.min(100).max(1);
    }

    fn capabilities(&self) -> EncoderCapabilities {
        EncoderCapabilities {
            codec_type: CodecType::Jpeg,
            hardware_accelerated: false,
            max_resolution: (8192, 8192),
            max_fps: 60,
        }
    }
}

/// Software JPEG decoder
pub struct JpegDecoder {
    config: DecoderConfig,
}

impl JpegDecoder {
    pub fn new(config: DecoderConfig) -> Self {
        Self { config }
    }
}

impl VideoDecoder for JpegDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> continuum_core::ContinuumResult<VideoFrame> {
        let img = image::load_from_memory(&frame.data)
            .map_err(|e| continuum_core::ContinuumError::Video(format!("JPEG decode error: {}", e)))?;
        
        let rgb = img.to_rgb8();
        
        Ok(VideoFrame {
            width: rgb.width(),
            height: rgb.height(),
            data: rgb.into_raw(),
            timestamp: frame.timestamp,
        })
    }
}

/// Create the best available encoder
pub fn create_encoder(config: EncoderConfig) -> Box<dyn VideoEncoder> {
    match config.codec_type {
        CodecType::Jpeg => Box::new(JpegEncoder::new(config)),
        _ => Box::new(JpegEncoder::new(config)), // Fallback to JPEG
    }
}

/// Create a decoder for the given codec type
pub fn create_decoder(config: DecoderConfig, codec_type: CodecType) -> Box<dyn VideoDecoder> {
    match codec_type {
        _ => Box::new(JpegDecoder::new(config)), // Fallback to JPEG
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_frame() -> VideoFrame {
        let width = 100;
        let height = 100;
        let data = vec![128u8; (width * height * 3) as usize];
        VideoFrame {
            width,
            height,
            data,
            timestamp: 0,
        }
    }

    #[test]
    fn test_jpeg_encode_decode() {
        let frame = create_test_frame();
        let config = EncoderConfig {
            codec_type: CodecType::Jpeg,
            width: 100,
            height: 100,
            quality: 80,
            ..Default::default()
        };
        
        let mut encoder = JpegEncoder::new(config);
        let encoded = encoder.encode(&frame).unwrap();
        assert_eq!(encoded.codec_type, CodecType::Jpeg);
        assert!(encoded.is_keyframe);
        assert!(!encoded.data.is_empty());

        let decoder_config = DecoderConfig { width: 100, height: 100 };
        let mut decoder = JpegDecoder::new(decoder_config);
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded.width, 100);
        assert_eq!(decoded.height, 100);
    }

    #[test]
    fn test_encoder_capabilities() {
        let config = EncoderConfig::default();
        let encoder = JpegEncoder::new(config);
        let caps = encoder.capabilities();
        assert_eq!(caps.codec_type, CodecType::Jpeg);
        assert!(!caps.hardware_accelerated);
    }

    #[test]
    fn test_create_encoder() {
        let config = EncoderConfig::default();
        let mut encoder = create_encoder(config);
        let frame = create_test_frame();
        let encoded = encoder.encode(&frame).unwrap();
        assert!(!encoded.data.is_empty());
    }
}
