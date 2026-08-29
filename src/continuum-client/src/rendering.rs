#![allow(dead_code)]

use continuum_transport::FrameSemantics;
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use std::time::Instant;

/// Shows the remote computer's screen in the main area.
/// Everything else is handled by the app — this just renders frames.
pub struct FrameRenderer {
    texture: Option<TextureHandle>,
    last_frame: Option<ColorImage>,
    frame_count: u64,
    fps: f32,
    last_fps_update: Instant,
    bytes_received: u64,
    current_quality: u8,
    resolution: (u32, u32),
    latency_ms: f32,
}

impl FrameRenderer {
    pub fn new() -> Self {
        Self {
            texture: None,
            last_frame: None,
            frame_count: 0,
            fps: 0.0,
            last_fps_update: Instant::now(),
            bytes_received: 0,
            current_quality: 85,
            resolution: (0, 0),
            latency_ms: 0.0,
        }
    }

    pub fn update_frame(&mut self, data: &[u8], semantics: &FrameSemantics) {
        self.frame_count += 1;
        self.bytes_received += data.len() as u64;
        self.current_quality = semantics.quality;
        self.resolution = (semantics.width, semantics.height);

        if data.is_empty() {
            return;
        }

        match image::load_from_memory(data) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels: Vec<Color32> = rgba
                    .pixels()
                    .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                    .collect();
                self.last_frame = Some(ColorImage { size, pixels });
                self.texture = None;
            }
            Err(err) => {
                tracing::warn!(error = %err, "Couldn't decode picture");
            }
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_fps_update).as_secs_f32();
        if elapsed > 0.5 {
            self.fps = self.fps * 0.7 + (1.0 / elapsed.max(0.001)) * 0.3;
            self.last_fps_update = now;
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui) -> Option<egui::Response> {
        let frame = self.last_frame.as_ref()?;

        if self.texture.is_none() {
            let tex = ui.ctx().load_texture(
                "remote-screen",
                frame.clone(),
                TextureOptions {
                    magnification: egui::TextureFilter::Linear,
                    minification: egui::TextureFilter::Linear,
                    ..Default::default()
                },
            );
            self.texture = Some(tex);
        }

        let texture = self.texture.as_ref()?;
        let available = ui.available_size();
        let tex_size = texture.size_vec2();
        let aspect = tex_size.x / tex_size.y;

        let (w, h) = if available.x / available.y > aspect {
            let h = available.y;
            (h * aspect, h)
        } else {
            let w = available.x;
            (w, w / aspect)
        };

        let response =
            ui.add(egui::Image::new((texture.id(), egui::vec2(w, h))).maintain_aspect_ratio(true));
        Some(response)
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }
    pub fn latency_ms(&self) -> f32 {
        self.latency_ms
    }
    pub fn has_frame(&self) -> bool {
        self.last_frame.is_some()
    }
    pub fn current_quality(&self) -> u8 {
        self.current_quality
    }
}
