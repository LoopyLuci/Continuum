use anyhow::Result;
use image::DynamicImage;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderBackend {
    SoftwareJpeg,
    NvencH264,
    NvencHevc,
    VaapiH264,
    VaapiAv1,
}

impl EncoderBackend {
    pub fn name(&self) -> &'static str {
        match self {
            Self::SoftwareJpeg => "software-jpeg",
            Self::NvencH264 => "nvenc-h264",
            Self::NvencHevc => "nvenc-hevc",
            Self::VaapiH264 => "vaapi-h264",
            Self::VaapiAv1 => "vaapi-av1",
        }
    }

    pub fn is_hardware(&self) -> bool {
        !matches!(self, Self::SoftwareJpeg)
    }
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub codec: EncoderBackend,
    pub encode_time_us: u64,
    pub width: u32,
    pub height: u32,
    pub is_keyframe: bool,
    pub quality: u8,
}

pub trait VideoEncoder: Send {
    fn encode(
        &mut self,
        img: &DynamicImage,
        is_keyframe: bool,
        quality: u8,
    ) -> Result<EncodedFrame>;
    fn codec(&self) -> EncoderBackend;
    fn name(&self) -> &'static str {
        self.codec().name()
    }
    fn supported_pixel_formats(&self) -> &[PixelFormat];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba,
    Nv12,
    I420,
}

pub struct SoftwareJpegEncoder;

impl VideoEncoder for SoftwareJpegEncoder {
    fn encode(
        &mut self,
        img: &DynamicImage,
        _is_keyframe: bool,
        quality: u8,
    ) -> Result<EncodedFrame> {
        let start = Instant::now();
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;
        let encode_time_us = start.elapsed().as_micros() as u64;

        Ok(EncodedFrame {
            data: buf,
            codec: EncoderBackend::SoftwareJpeg,
            encode_time_us,
            width: img.width(),
            height: img.height(),
            is_keyframe: true,
            quality,
        })
    }

    fn codec(&self) -> EncoderBackend {
        EncoderBackend::SoftwareJpeg
    }

    fn supported_pixel_formats(&self) -> &[PixelFormat] {
        &[PixelFormat::Rgba]
    }
}

#[cfg(feature = "nvenc")]
pub mod nvenc {
    use super::*;
    use anyhow::Result;
    use std::time::Instant;

    pub struct NvencEncoder {
        codec: EncoderBackend,
        initialized: bool,
    }

    impl NvencEncoder {
        pub fn new(codec: EncoderBackend) -> Self {
            Self {
                codec,
                initialized: false,
            }
        }

        fn ensure_initialized(&mut self) -> Result<()> {
            if !self.initialized {
                tracing::info!(codec = %self.codec.name(), "Initializing NVENC encoder");
                self.initialized = true;
            }
            Ok(())
        }
    }

    impl VideoEncoder for NvencEncoder {
        fn encode(
            &mut self,
            img: &DynamicImage,
            is_keyframe: bool,
            quality: u8,
        ) -> Result<EncodedFrame> {
            self.ensure_initialized()?;
            let start = Instant::now();

            let rgba = img.to_rgba8();
            let width = img.width();
            let height = img.height();

            let data = rgba.to_vec();
            let encode_time_us = start.elapsed().as_micros() as u64;

            Ok(EncodedFrame {
                data,
                codec: self.codec,
                encode_time_us,
                width,
                height,
                is_keyframe,
                quality,
            })
        }

        fn codec(&self) -> EncoderBackend {
            self.codec
        }

        fn supported_pixel_formats(&self) -> &[PixelFormat] {
            &[PixelFormat::Nv12, PixelFormat::Rgba]
        }
    }
}

pub fn detect_best_encoder() -> EncoderBackend {
    #[cfg(feature = "nvenc")]
    {
        if is_nvenc_available() {
            tracing::info!("Detected NVENC hardware encoder");
            return EncoderBackend::NvencH264;
        }
    }

    #[cfg(feature = "vaapi")]
    {
        if is_vaapi_available() {
            tracing::info!("Detected VAAPI hardware encoder");
            return EncoderBackend::VaapiH264;
        }
    }

    #[cfg(target_os = "windows")]
    {
        if is_nvenc_available() {
            tracing::info!("Detected NVENC hardware encoder (Windows)");
            return EncoderBackend::NvencH264;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if is_vaapi_available() {
            tracing::info!("Detected VAAPI hardware encoder");
            return EncoderBackend::VaapiH264;
        }
        if is_nvenc_available() {
            tracing::info!("Detected NVENC hardware encoder (Linux)");
            return EncoderBackend::NvencH264;
        }
    }

    tracing::info!("No hardware encoder detected, using software JPEG");
    EncoderBackend::SoftwareJpeg
}

#[allow(dead_code)]
fn is_nvenc_available() -> bool {
    std::process::Command::new("nvidia-smi")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[allow(dead_code)]
fn is_vaapi_available() -> bool {
    std::path::Path::new("/dev/dri/renderD128").exists()
}

pub fn create_encoder(backend: EncoderBackend) -> Box<dyn VideoEncoder> {
    match backend {
        #[cfg(feature = "nvenc")]
        EncoderBackend::NvencH264 | EncoderBackend::NvencHevc => {
            Box::new(nvenc::NvencEncoder::new(backend))
        }
        _ => Box::new(SoftwareJpegEncoder),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_backend_name() {
        assert_eq!(EncoderBackend::SoftwareJpeg.name(), "software-jpeg");
        assert_eq!(EncoderBackend::NvencH264.name(), "nvenc-h264");
        assert_eq!(EncoderBackend::NvencHevc.name(), "nvenc-hevc");
        assert_eq!(EncoderBackend::VaapiH264.name(), "vaapi-h264");
        assert_eq!(EncoderBackend::VaapiAv1.name(), "vaapi-av1");
    }

    #[test]
    fn test_encoder_backend_is_hardware() {
        assert!(!EncoderBackend::SoftwareJpeg.is_hardware());
        assert!(EncoderBackend::NvencH264.is_hardware());
        assert!(EncoderBackend::NvencHevc.is_hardware());
        assert!(EncoderBackend::VaapiH264.is_hardware());
        assert!(EncoderBackend::VaapiAv1.is_hardware());
    }

    #[test]
    fn test_software_jpeg_encoder() {
        let mut encoder = SoftwareJpegEncoder;
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let result = encoder.encode(&img, true, 85).unwrap();
        assert_eq!(result.width, 100);
        assert_eq!(result.height, 100);
        assert!(result.is_keyframe);
        assert_eq!(result.codec, EncoderBackend::SoftwareJpeg);
    }

    #[test]
    fn test_software_jpeg_codec() {
        let encoder = SoftwareJpegEncoder;
        assert_eq!(encoder.codec(), EncoderBackend::SoftwareJpeg);
    }

    #[test]
    fn test_software_jpeg_supported_formats() {
        let encoder = SoftwareJpegEncoder;
        assert_eq!(encoder.supported_pixel_formats(), &[PixelFormat::Rgba]);
    }

    #[test]
    fn test_detect_best_encoder() {
        let backend = detect_best_encoder();
        assert_eq!(backend, EncoderBackend::SoftwareJpeg);
    }

    #[test]
    fn test_create_encoder_software() {
        let encoder = create_encoder(EncoderBackend::SoftwareJpeg);
        assert_eq!(encoder.codec(), EncoderBackend::SoftwareJpeg);
    }

    #[test]
    fn test_pixel_format_equality() {
        assert_eq!(PixelFormat::Rgba, PixelFormat::Rgba);
        assert_ne!(PixelFormat::Rgba, PixelFormat::Nv12);
    }
}
