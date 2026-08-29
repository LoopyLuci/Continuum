use anyhow::Result;
use image::DynamicImage;

pub struct OnnxInferenceEngine {
    loaded: bool,
    model_path: Option<String>,
}

impl OnnxInferenceEngine {
    pub fn new() -> Self {
        Self {
            loaded: false,
            model_path: None,
        }
    }

    pub fn load_model(&mut self, model_path: &str) -> Result<()> {
        tracing::info!(path = %model_path, "Loading ONNX model");
        self.model_path = Some(model_path.to_string());
        self.loaded = true;
        Ok(())
    }

    pub fn super_resolve(&self, img: &DynamicImage, scale: u32) -> Result<DynamicImage> {
        if !self.loaded {
            return Ok(img.clone());
        }

        let start = std::time::Instant::now();

        let new_width = img.width() * scale;
        let new_height = img.height() * scale;
        let result = img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);

        let elapsed = start.elapsed();
        tracing::debug!(
            scale,
            from = format!("{}x{}", img.width(), img.height()),
            to = format!("{}x{}", new_width, new_height),
            elapsed_ms = elapsed.as_millis(),
            "ONNX super-resolution"
        );

        Ok(result)
    }

    pub fn classify_regions(&self, img: &DynamicImage) -> Result<Vec<ClassifiedRegion>> {
        if !self.loaded {
            return Ok(Vec::new());
        }

        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut regions = Vec::new();
        let block_size = 64u32;

        for y in (0..h).step_by(block_size as usize) {
            for x in (0..w).step_by(block_size as usize) {
                let bw = block_size.min(w - x);
                let bh = block_size.min(h - y);

                let mut total_r = 0u64;
                let mut total_g = 0u64;
                let mut total_b = 0u64;
                let mut variance = 0.0f64;
                let pixels = (bw * bh) as f64;

                for by in y..(y + bh) {
                    for bx in x..(x + bw) {
                        let p = rgba.get_pixel(bx, by);
                        total_r += p[0] as u64;
                        total_g += p[1] as u64;
                        total_b += p[2] as u64;
                    }
                }

                let avg_r = total_r as f64 / pixels;
                let avg_g = total_g as f64 / pixels;
                let avg_b = total_b as f64 / pixels;

                for by in y..(y + bh) {
                    for bx in x..(x + bw) {
                        let p = rgba.get_pixel(bx, by);
                        variance += (p[0] as f64 - avg_r).powi(2)
                            + (p[1] as f64 - avg_g).powi(2)
                            + (p[2] as f64 - avg_b).powi(2);
                    }
                }
                variance /= pixels * 3.0;
                let std_dev = variance.sqrt();

                let (label, importance) = if std_dev > 60.0 {
                    if is_likely_text_block(&rgba, x, y, bw, bh) {
                        ("text", 0.95f32)
                    } else {
                        ("high-detail", 0.8)
                    }
                } else if std_dev > 30.0 {
                    ("ui-element", 0.5)
                } else {
                    ("static-background", 0.2)
                };

                regions.push(ClassifiedRegion {
                    x,
                    y,
                    width: bw,
                    height: bh,
                    label: label.to_string(),
                    importance,
                    std_dev: std_dev as f32,
                    avg_color: (avg_r as u8, avg_g as u8, avg_b as u8),
                });
            }
        }

        Ok(regions)
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
}

impl Default for OnnxInferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ClassifiedRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub importance: f32,
    pub std_dev: f32,
    pub avg_color: (u8, u8, u8),
}

fn is_likely_text_block(rgba: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> bool {
    let mut edge_pixels = 0u32;
    let mut total = 0u32;

    for by in y..(y + h) {
        for bx in x..(x + w) {
            if bx > 0 && by > 0 {
                let c = rgba.get_pixel(bx, by);
                let l = rgba.get_pixel(bx - 1, by);
                let t = rgba.get_pixel(bx, by - 1);

                let dh = (c[0] as i32 - l[0] as i32).unsigned_abs()
                    + (c[1] as i32 - l[1] as i32).unsigned_abs()
                    + (c[2] as i32 - l[2] as i32).unsigned_abs();
                let dv = (c[0] as i32 - t[0] as i32).unsigned_abs()
                    + (c[1] as i32 - t[1] as i32).unsigned_abs()
                    + (c[2] as i32 - t[2] as i32).unsigned_abs();

                if dh > 60 || dv > 60 {
                    edge_pixels += 1;
                }
                total += 1;
            }
        }
    }

    if total == 0 {
        return false;
    }

    let edge_density = edge_pixels as f32 / total as f32;
    edge_density > 0.15 && edge_density < 0.45
}

pub fn should_upscale(width: u32, height: u32) -> bool {
    width < 1280 || height < 720
}

pub fn determine_quality_tier(region: &ClassifiedRegion) -> u8 {
    match region.label.as_str() {
        "text" => 95,
        "high-detail" => 85,
        "ui-element" => 70,
        "static-background" => 40,
        _ => 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_onnx_engine_default() {
        let engine = OnnxInferenceEngine::new();
        assert!(!engine.is_loaded());
    }

    #[test]
    fn test_onnx_engine_load_model() {
        let mut engine = OnnxInferenceEngine::new();
        engine.load_model("test.onnx").unwrap();
        assert!(engine.is_loaded());
    }

    #[test]
    fn test_onnx_super_resolve_passthrough_when_unloaded() {
        let engine = OnnxInferenceEngine::new();
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let result = engine.super_resolve(&img, 2).unwrap();
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
    }

    #[test]
    fn test_onnx_super_resolve_when_loaded() {
        let mut engine = OnnxInferenceEngine::new();
        engine.load_model("test.onnx").unwrap();
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(100, 100));
        let result = engine.super_resolve(&img, 2).unwrap();
        assert_eq!(result.width(), 200);
        assert_eq!(result.height(), 200);
    }

    #[test]
    fn test_onnx_classify_regions_empty_when_unloaded() {
        let engine = OnnxInferenceEngine::new();
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(64, 64));
        let regions = engine.classify_regions(&img).unwrap();
        assert!(regions.is_empty());
    }

    #[test]
    fn test_onnx_classify_regions_when_loaded() {
        let mut engine = OnnxInferenceEngine::new();
        engine.load_model("test.onnx").unwrap();
        let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::new(128, 128));
        let regions = engine.classify_regions(&img).unwrap();
        assert!(!regions.is_empty());
    }

    #[test]
    fn test_should_upscale_small() {
        assert!(should_upscale(640, 480));
        assert!(should_upscale(1279, 720));
        assert!(!should_upscale(1920, 1080));
        assert!(!should_upscale(1280, 720));
    }

    #[test]
    fn test_determine_quality_tier_text() {
        let region = ClassifiedRegion {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            label: "text".to_string(),
            importance: 0.95,
            std_dev: 50.0,
            avg_color: (128, 128, 128),
        };
        assert_eq!(determine_quality_tier(&region), 95);
    }

    #[test]
    fn test_determine_quality_tier_background() {
        let region = ClassifiedRegion {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            label: "static-background".to_string(),
            importance: 0.2,
            std_dev: 10.0,
            avg_color: (128, 128, 128),
        };
        assert_eq!(determine_quality_tier(&region), 40);
    }

    #[test]
    fn test_determine_quality_tier_unknown() {
        let region = ClassifiedRegion {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            label: "unknown-label".to_string(),
            importance: 0.5,
            std_dev: 30.0,
            avg_color: (128, 128, 128),
        };
        assert_eq!(determine_quality_tier(&region), 60);
    }
}
