use crate::error::{ensure, ContinuumError, ContinuumResult};
use crate::types::{ContentType, FrameSemantics};
use image::DynamicImage;
use std::io::Cursor;
use std::time::Instant;
use tracing;

pub struct AdaptiveEncoder {
    quality: u8,
    target_encode_time_us: u64,
    frame_number: u64,
    last_frame_data: Option<Vec<u8>>,
    consecutive_similar: u32,
    keyframe_interval: u32,
    frames_since_keyframe: u32,
}

impl AdaptiveEncoder {
    pub fn new(quality: u8, target_fps: u32) -> Self {
        let safe_fps = target_fps.clamp(1, 240);
        Self {
            quality: quality.clamp(1, 100),
            target_encode_time_us: 1_000_000 / safe_fps as u64 / 2,
            frame_number: 0,
            last_frame_data: None,
            consecutive_similar: 0,
            keyframe_interval: 30,
            frames_since_keyframe: 0,
        }
    }

    pub fn encode(
        &mut self,
        img: &DynamicImage,
        monitor_id: u32,
    ) -> ContinuumResult<(Vec<u8>, FrameSemantics)> {
        ensure(
            img.width() > 0 && img.height() > 0,
            "Cannot encode zero-dimension image",
        )?;
        let start = Instant::now();
        self.frame_number += 1;
        self.frames_since_keyframe += 1;

        let rgba = img.to_rgba8();
        let raw_data = rgba.as_raw();
        let frame_len = raw_data.len();

        let similarity = self
            .last_frame_data
            .as_ref()
            .map(|prev| {
                let cmp_len = prev.len().min(frame_len);
                if cmp_len == 0 {
                    return 0.0;
                }
                let diff = prev[..cmp_len]
                    .iter()
                    .zip(raw_data[..cmp_len].iter())
                    .filter(|(a, b)| {
                        let d = (**a).abs_diff(**b);
                        d > 4
                    })
                    .count();
                1.0 - (diff as f32 / cmp_len as f32)
            })
            .unwrap_or(0.0);

        let is_keyframe = self.frames_since_keyframe >= self.keyframe_interval || similarity < 0.85;

        if similarity > 0.98 && self.consecutive_similar > 10 && !is_keyframe {
            self.consecutive_similar += 1;
            let semantics = FrameSemantics {
                content_type: ContentType::Jpeg,
                width: img.width(),
                height: img.height(),
                quality: self.quality,
                frame_number: self.frame_number,
                timestamp: chrono::Utc::now(),
                is_keyframe: false,
                monitor_id,
                encode_time_us: 0,
            };
            return Ok((Vec::new(), semantics));
        }

        if similarity > 0.98 {
            self.consecutive_similar += 1;
        } else {
            self.consecutive_similar = 0;
        }

        if is_keyframe {
            self.frames_since_keyframe = 0;
        }

        let effective_quality = if is_keyframe {
            self.quality
        } else {
            (self.quality as u32 * 70 / 100).max(30) as u8
        };

        let encoded =
            encode_jpeg(img, effective_quality).map_err(|e| ContinuumError::EncodeFailed {
                backend: "jpeg".into(),
                reason: e.to_string(),
            })?;
        let encode_time = start.elapsed().as_micros() as u64;

        if encode_time > self.target_encode_time_us * 2 && self.quality > 30 {
            self.quality = (self.quality - 5).max(30);
            tracing::debug!(
                new_quality = self.quality,
                "Reduced quality due to slow encode"
            );
        } else if encode_time < self.target_encode_time_us / 2 && self.quality < 95 {
            self.quality = (self.quality + 2).min(95);
        }

        self.last_frame_data = Some(raw_data.to_vec());

        let semantics = FrameSemantics {
            content_type: ContentType::Jpeg,
            width: img.width(),
            height: img.height(),
            quality: effective_quality,
            frame_number: self.frame_number,
            timestamp: chrono::Utc::now(),
            is_keyframe,
            monitor_id,
            encode_time_us: encode_time,
        };

        Ok((encoded, semantics))
    }

    pub fn set_quality(&mut self, quality: u8) {
        self.quality = quality.clamp(1, 100);
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    pub fn frame_number(&self) -> u64 {
        self.frame_number
    }
}

pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> ContinuumResult<Vec<u8>> {
    let quality = quality.clamp(1, 100);
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut buf = Vec::new();
    {
        let mut cursor = Cursor::new(&mut buf);
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
        let mut encoder = encoder;
        encoder
            .encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
            .map_err(|e| ContinuumError::EncodeFailed {
                backend: "jpeg".into(),
                reason: e.to_string(),
            })?;
    }
    if buf.is_empty() {
        return Err(ContinuumError::EncodeFailed {
            backend: "jpeg".into(),
            reason: "Encoder produced empty output".into(),
        });
    }
    Ok(buf)
}

pub fn decode_jpeg(data: &[u8]) -> ContinuumResult<DynamicImage> {
    if data.is_empty() {
        return Err(ContinuumError::DecodeFailed("Empty input data".into()));
    }
    if data.len() < 3 {
        return Err(ContinuumError::DecodeFailed(format!(
            "Input too small for JPEG: {} bytes",
            data.len()
        )));
    }
    image::load_from_memory(data)
        .map_err(|e| ContinuumError::DecodeFailed(format!("JPEG decode error: {}", e)))
}

pub fn capture_encode_with_semantics(
    encoder: &mut AdaptiveEncoder,
    monitor_id: u32,
) -> ContinuumResult<(Vec<u8>, FrameSemantics)> {
    let img = crate::capture::capture_monitor(monitor_id)?;
    encoder.encode(&img, monitor_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_encoder_initial_quality() {
        let encoder = AdaptiveEncoder::new(85, 30);
        assert_eq!(encoder.quality(), 85);
        assert_eq!(encoder.frame_number(), 0);
    }

    #[test]
    fn test_encoder_clamps_quality() {
        let encoder = AdaptiveEncoder::new(200, 30);
        assert_eq!(encoder.quality(), 100);
        let encoder = AdaptiveEncoder::new(0, 30);
        assert_eq!(encoder.quality(), 1);
    }

    #[test]
    fn test_encoder_clamps_fps() {
        let encoder = AdaptiveEncoder::new(85, 0);
        assert_eq!(encoder.frame_number(), 0);
        let encoder = AdaptiveEncoder::new(85, 500);
        assert_eq!(encoder.frame_number(), 0);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let img = crate::capture::synthesize_demo_frame();
        let encoded = encode_jpeg(&img, 85).unwrap();
        assert!(!encoded.is_empty());
        let decoded = decode_jpeg(&encoded).unwrap();
        assert_eq!(decoded.width(), img.width());
        assert_eq!(decoded.height(), img.height());
    }

    #[test]
    fn test_decode_empty_fails() {
        assert!(decode_jpeg(&[]).is_err());
    }

    use crate::capture::compute_frame_diff;

    #[test]
    fn test_frame_diff_identical() {
        let data = vec![128u8; 100];
        let diff = compute_frame_diff(&data, &data);
        assert_eq!(diff, 0.0);
    }

    #[test]
    fn test_frame_diff_different() {
        let a = vec![0u8; 100];
        let b = vec![255u8; 100];
        let diff = compute_frame_diff(&a, &b);
        assert_eq!(diff, 1.0);
    }

    #[test]
    fn test_frame_diff_mismatched_sizes() {
        let a = vec![0u8; 100];
        let b = vec![0u8; 50];
        let diff = compute_frame_diff(&a, &b);
        assert_eq!(diff, 1.0);
    }
}
