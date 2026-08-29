use anyhow::Result;
use continuum_transport::types::SemanticRegion;
use image::DynamicImage;
use tracing;

#[allow(dead_code)]
pub struct Segmenter {
    min_region_size: u32,
    threshold: f32,
}

impl Segmenter {
    pub fn new() -> Self {
        Self {
            min_region_size: 50,
            threshold: 0.3,
        }
    }

    pub fn detect_regions(&self, img: &DynamicImage) -> Result<Vec<SemanticRegion>> {
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();

        let mut regions = Vec::new();

        let block_size = 32;
        for y in (0..height).step_by(block_size as usize) {
            for x in (0..width).step_by(block_size as usize) {
                let block_w = block_size.min(width - x);
                let block_h = block_size.min(height - y);

                let mut edge_count = 0u32;
                let mut total_pixels = 0u32;

                for by in y..(y + block_h) {
                    for bx in x..(x + block_w) {
                        if bx > 0 && by > 0 {
                            let curr = rgba.get_pixel(bx, by);
                            let left = rgba.get_pixel(bx - 1, by);
                            let top = rgba.get_pixel(bx, by - 1);

                            let diff_h = ((curr[0] as i32 - left[0] as i32).abs()
                                + (curr[1] as i32 - left[1] as i32).abs()
                                + (curr[2] as i32 - left[2] as i32).abs())
                                as f32
                                / 3.0;

                            let diff_v = ((curr[0] as i32 - top[0] as i32).abs()
                                + (curr[1] as i32 - top[1] as i32).abs()
                                + (curr[2] as i32 - top[2] as i32).abs())
                                as f32
                                / 3.0;

                            if diff_h > 30.0 || diff_v > 30.0 {
                                edge_count += 1;
                            }
                        }
                        total_pixels += 1;
                    }
                }

                let edge_density = edge_count as f32 / total_pixels as f32;
                if edge_density > self.threshold {
                    let importance = (edge_density * 2.0).min(1.0);
                    regions.push(SemanticRegion {
                        x,
                        y,
                        width: block_w,
                        height: block_h,
                        label: "high-detail".to_string(),
                        importance,
                    });
                }
            }
        }

        tracing::debug!(count = regions.len(), "Detected semantic regions");
        Ok(regions)
    }

    pub fn classify_region(region: &SemanticRegion) -> &'static str {
        if region.importance > 0.7 {
            "text"
        } else if region.importance > 0.4 {
            "ui-element"
        } else {
            "background"
        }
    }
}

impl Default for Segmenter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segmenter_returns_empty_for_gradient() {
        let segmenter = Segmenter::new();
        let img = continuum_transport::capture::synthesize_demo_frame();
        let regions = segmenter.detect_regions(&img).unwrap();
        assert!(regions.is_empty());
    }

    #[test]
    fn test_segmenter_detects_edges() {
        let segmenter = Segmenter::new();
        let mut img = image::ImageBuffer::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let val = if (x / 4 + y / 4) % 2 == 0 { 255u8 } else { 0u8 };
                img.put_pixel(x, y, image::Rgba([val, val, val, 255]));
            }
        }
        let img = image::DynamicImage::ImageRgba8(img);
        let regions = segmenter.detect_regions(&img).unwrap();
        assert!(!regions.is_empty());
    }
}
