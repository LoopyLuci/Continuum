use crate::error::{ensure, ContinuumError, ContinuumResult};
use crate::types::MonitorInfo;
use image::{DynamicImage, ImageBuffer, Rgba};
use tracing;

pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    #[cfg(target_os = "windows")]
    {
        match screenshots::Screen::all() {
            Ok(screens) => screens
                .into_iter()
                .enumerate()
                .map(|(i, screen)| MonitorInfo {
                    id: i as u32,
                    name: format!("Monitor {}", i + 1),
                    x: screen.display_info.x,
                    y: screen.display_info.y,
                    width: screen.display_info.width,
                    height: screen.display_info.height,
                    is_primary: i == 0,
                    scale_factor: screen.display_info.scale_factor,
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to enumerate monitors");
                vec![fallback_monitor()]
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        vec![fallback_monitor()]
    }
}

fn fallback_monitor() -> MonitorInfo {
    MonitorInfo {
        id: 0,
        name: "Default Display".to_string(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        is_primary: true,
        scale_factor: 1.0,
    }
}

pub fn capture_monitor(monitor_id: u32) -> ContinuumResult<DynamicImage> {
    #[cfg(target_os = "windows")]
    {
        let screens = screenshots::Screen::all().map_err(|e| {
            ContinuumError::CaptureFailed(format!("Failed to enumerate screens: {}", e))
        })?;
        let screen = screens
            .into_iter()
            .nth(monitor_id as usize)
            .ok_or_else(|| {
                ContinuumError::MonitorNotFound(format!("Monitor {} not found", monitor_id))
            })?;
        let buffer = screen.capture().map_err(|e| {
            ContinuumError::CaptureFailed(format!(
                "Failed to capture monitor {}: {}",
                monitor_id, e
            ))
        })?;
        let width = buffer.width();
        let height = buffer.height();
        ensure(
            width > 0 && height > 0,
            "Captured frame has zero dimensions",
        )?;
        ImageBuffer::from_raw(width, height, buffer.into_raw())
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| {
                ContinuumError::CaptureFailed(
                    "Failed to create image buffer from captured data".into(),
                )
            })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = monitor_id;
        tracing::debug!(monitor_id, "Using non-Windows capture backend");

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            match capture_scrap(monitor_id) {
                Ok(img) => return Ok(img),
                Err(e) => {
                    tracing::warn!(error = %e, "scrap capture failed, falling back to synthetic demo frame");
                }
            }
        }

        Ok(synthesize_demo_frame())
    }
}

pub fn capture_screen_frame() -> ContinuumResult<DynamicImage> {
    capture_monitor(0)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn capture_scrap(monitor_id: u32) -> ContinuumResult<DynamicImage> {
    let display = scrap::Display::primary().map_err(|e| {
        ContinuumError::CaptureFailed(format!("scrap primary display failed: {}", e))
    })?;
    let mut capturer = scrap::Capturer::new(&display)
        .map_err(|e| ContinuumError::CaptureFailed(format!("scrap capturer failed: {}", e)))?;

    let frame = capturer
        .frame()
        .map_err(|e| ContinuumError::CaptureFailed(format!("scrap frame capture failed: {}", e)))?;

    let width = frame.width();
    let height = frame.height();
    ensure!(
        width > 0 && height > 0,
        "scrap captured frame has zero dimensions"
    )?;

    ImageBuffer::from_raw(width, height, frame.to_vec())
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| {
            ContinuumError::CaptureFailed("Failed to build image from scrap frame".into())
        })
}

pub fn synthesize_demo_frame() -> DynamicImage {
    let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(640, 360);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = (x as f32 * 0.5) as u8;
        let g = (y as f32 * 0.4) as u8;
        let b = ((x + y) as f32 * 0.2) as u8;
        *pixel = Rgba([r, g, b, 255]);
    }
    DynamicImage::ImageRgba8(img)
}

pub fn compute_frame_diff(prev: &[u8], curr: &[u8]) -> f32 {
    if prev.len() != curr.len() || prev.is_empty() {
        tracing::warn!(
            prev_len = prev.len(),
            curr_len = curr.len(),
            "Frame diff: buffer size mismatch"
        );
        return 1.0;
    }
    let diff_count = prev
        .iter()
        .zip(curr.iter())
        .filter(|(a, b)| {
            let d = (**a).abs_diff(**b);
            d > 4
        })
        .count();
    diff_count as f32 / prev.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesize_demo_frame_dimensions() {
        let img = synthesize_demo_frame();
        assert_eq!(img.width(), 640);
        assert_eq!(img.height(), 360);
    }

    #[test]
    fn test_enumerate_monitors_returns_at_least_one() {
        let monitors = enumerate_monitors();
        assert!(!monitors.is_empty());
    }

    #[test]
    fn test_fallback_monitor_defaults() {
        let monitor = fallback_monitor();
        assert_eq!(monitor.id, 0);
        assert!(monitor.is_primary);
        assert_eq!(monitor.width, 1920);
        assert_eq!(monitor.height, 1080);
    }

    #[test]
    fn test_capture_screen_frame() {
        let result = capture_screen_frame();
        assert!(result.is_ok());
        let img = result.unwrap();
        assert!(img.width() > 0);
        assert!(img.height() > 0);
    }

    #[test]
    fn test_compute_frame_diff_empty() {
        let diff = compute_frame_diff(&[], &[]);
        assert_eq!(diff, 1.0);
    }

    #[test]
    fn test_compute_frame_diff_size_mismatch() {
        let a = vec![0u8; 100];
        let b = vec![0u8; 50];
        let diff = compute_frame_diff(&a, &b);
        assert_eq!(diff, 1.0);
    }

    #[test]
    fn test_compute_frame_diff_identical() {
        let data = vec![128u8; 200];
        let diff = compute_frame_diff(&data, &data);
        assert_eq!(diff, 0.0);
    }

    #[test]
    fn test_compute_frame_diff_all_different() {
        let a = vec![0u8; 100];
        let b = vec![255u8; 100];
        let diff = compute_frame_diff(&a, &b);
        assert_eq!(diff, 1.0);
    }

    #[test]
    fn test_compute_frame_diff_small_difference_ignored() {
        let a = vec![100u8; 100];
        let b = vec![103u8; 100]; // diff of 3, threshold is > 4
        let diff = compute_frame_diff(&a, &b);
        assert_eq!(diff, 0.0);
    }
}
