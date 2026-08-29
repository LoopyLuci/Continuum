use anyhow::Result;
use image::DynamicImage;
use tracing;

#[allow(dead_code)]
pub struct SuperResolver {
    scale_factor: u32,
    quality_threshold: f32,
}

impl SuperResolver {
    pub fn new(scale_factor: u32) -> Self {
        Self {
            scale_factor: scale_factor.clamp(2, 4),
            quality_threshold: 0.5,
        }
    }

    pub fn upscale(&self, img: &DynamicImage) -> Result<DynamicImage> {
        let start = std::time::Instant::now();

        let new_width = img.width() * self.scale_factor;
        let new_height = img.height() * self.scale_factor;

        let upscaled =
            img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);

        let elapsed = start.elapsed();
        tracing::debug!(
            scale = self.scale_factor,
            from = format!("{}x{}", img.width(), img.height()),
            to = format!("{}x{}", new_width, new_height),
            elapsed_ms = elapsed.as_millis(),
            "Super-resolution upscale"
        );

        Ok(upscaled)
    }

    pub fn should_upscale(&self, img: &DynamicImage) -> bool {
        img.width() < 1280 || img.height() < 720
    }

    pub fn scale_factor(&self) -> u32 {
        self.scale_factor
    }
}

impl Default for SuperResolver {
    fn default() -> Self {
        Self::new(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upscale_2x() {
        let resolver = SuperResolver::new(2);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let upscaled = resolver.upscale(&img).unwrap();
        assert_eq!(upscaled.width(), 200);
        assert_eq!(upscaled.height(), 200);
    }

    #[test]
    fn test_should_upscale_small() {
        let resolver = SuperResolver::default();
        let small = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(640, 480));
        assert!(resolver.should_upscale(&small));

        let large = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(1920, 1080));
        assert!(!resolver.should_upscale(&large));
    }
}
