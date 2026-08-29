use crate::onnx_engine::{should_upscale, ClassifiedRegion, OnnxInferenceEngine};
use image::DynamicImage;

pub struct AiMediaEnhancer {
    engine: OnnxInferenceEngine,
    enabled: bool,
    upscale_enabled: bool,
    classification_enabled: bool,
}

impl AiMediaEnhancer {
    pub fn new() -> Self {
        let mut engine = OnnxInferenceEngine::new();
        let _ = engine.load_model("models/real-esrgan.onnx");
        Self {
            engine,
            enabled: true,
            upscale_enabled: false,
            classification_enabled: true,
        }
    }

    pub fn classify_and_optimize(
        &mut self,
        img: &DynamicImage,
        base_quality: u8,
    ) -> Result<ClassificationResult, anyhow::Error> {
        if !self.enabled || !self.classification_enabled {
            return Ok(ClassificationResult {
                regions: Vec::new(),
                effective_quality: base_quality,
                should_upscale: false,
            });
        }

        let regions = self.engine.classify_regions(img)?;

        let min_importance = regions
            .iter()
            .map(|r| r.importance)
            .fold(1.0f32, |a, b| a.min(b));

        let quality_tier = if min_importance > 0.8 {
            base_quality
        } else if min_importance > 0.5 {
            (base_quality as f32 * 0.85) as u8
        } else {
            (base_quality as f32 * 0.6).max(30.0) as u8
        };

        tracing::debug!(
            regions = regions.len(),
            min_importance = min_importance,
            base_quality = base_quality,
            effective_quality = quality_tier,
            "AI classification applied"
        );

        Ok(ClassificationResult {
            regions,
            effective_quality: quality_tier,
            should_upscale: self.upscale_enabled && should_upscale(img.width(), img.height()),
        })
    }

    pub fn upscale_if_needed(&mut self, img: &DynamicImage) -> DynamicImage {
        if !self.enabled || !self.upscale_enabled {
            return img.clone();
        }

        if should_upscale(img.width(), img.height()) {
            match self.engine.super_resolve(img, 2) {
                Ok(upscaled) => upscaled,
                Err(_) => img.clone(),
            }
        } else {
            img.clone()
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn set_upscale(&mut self, enabled: bool) {
        self.upscale_enabled = enabled;
    }
    pub fn set_classification(&mut self, enabled: bool) {
        self.classification_enabled = enabled;
    }
}

impl Default for AiMediaEnhancer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ClassificationResult {
    pub regions: Vec<ClassifiedRegion>,
    pub effective_quality: u8,
    pub should_upscale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ai_enhancer_default() {
        let enhancer = AiMediaEnhancer::new();
        assert!(enhancer.enabled);
        assert!(!enhancer.upscale_enabled);
        assert!(enhancer.classification_enabled);
    }

    #[test]
    fn test_ai_enhancer_classify_disabled() {
        let mut enhancer = AiMediaEnhancer::new();
        enhancer.set_enabled(false);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let result = enhancer.classify_and_optimize(&img, 85).unwrap();
        assert!(result.regions.is_empty());
        assert_eq!(result.effective_quality, 85);
    }

    #[test]
    fn test_ai_enhancer_classification_disabled() {
        let mut enhancer = AiMediaEnhancer::new();
        enhancer.set_classification(false);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let result = enhancer.classify_and_optimize(&img, 85).unwrap();
        assert!(result.regions.is_empty());
    }

    #[test]
    fn test_ai_enhancer_upscale_passthrough_when_disabled() {
        let mut enhancer = AiMediaEnhancer::new();
        enhancer.set_upscale(false);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(640, 480));
        let result = enhancer.upscale_if_needed(&img);
        assert_eq!(result.width(), 640);
        assert_eq!(result.height(), 480);
    }

    #[test]
    fn test_ai_enhancer_upscale_when_enabled() {
        let mut enhancer = AiMediaEnhancer::new();
        enhancer.set_upscale(true);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(640, 480));
        let result = enhancer.upscale_if_needed(&img);
        assert_eq!(result.width(), 1280);
        assert_eq!(result.height(), 960);
    }

    #[test]
    fn test_ai_enhancer_upscale_large_image_noop() {
        let mut enhancer = AiMediaEnhancer::new();
        enhancer.set_upscale(true);
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(1920, 1080));
        let result = enhancer.upscale_if_needed(&img);
        assert_eq!(result.width(), 1920);
    }
}
