#![allow(unexpected_cfgs)]

//! # Continuum — GPU Encoding & Rendering Pipeline
//!
//! This module covers the **Phase 14-15** GPU-accelerated encoding and
//! modern rendering pipeline. It documents how to build and integrate
//! the ffmpeg-next NVENC/VAAPI backend and the wgpu-based compositor.

// ============================================================================
// Phase 14: Real GPU Encoding (NVENC/VAAPI)
// ============================================================================

//! ## Building with GPU Support
//!
//! ```bash
//! # With ffmpeg (NVENC, VAAPI, H.265/HEVC decode)
//! cargo build --features "ffmpeg" --bin continuum-server
//!
//! # With NVENC specifically (Windows)
//! cargo build --features "nvenc" --bin continuum-server
//! ```
//!
//! ## Encoder Selection Logic
//!
//! The server auto-selects the best encoder at startup:
//!
//! 1. Detect NVENC (NVIDIA) → `hevc_nvenc` encoder
//! 2. Detect VAAPI (Intel/AMD) → `h264_vaapi` encoder  
//! 3. Fallback → software JPEG (always available)
//!
//! ```ignore
//! let backend = detect_best_encoder();           // auto-detect
//! let mut encoder = create_encoder(backend);     // create encoder
//! let frame = encoder.encode(&img, is_key, 85);  // encode frame
//! ```
//!
//! ## Client-Side Hardware Decode
//!
//! Windows: Direct3D11 + MediaFoundation (via ffmpeg dxva2)
//! macOS: VideoToolbox (built-in H.265 decoder)
//! Linux: VDPAU or VAAPI
//! Web: WebCodecs (browser-provided)

// ============================================================================
// Phase 15: wgpu Rendering Pipeline
// ============================================================================

//! ## Architecture
//!
//! The wgpu-based renderer replaces the current egui CPU-side texture upload
//! with a dedicated GPU compositor running on a separate thread.
//!
//! ```text
//! [Network Thread] → Frame Queue → [Render Thread] → wgpu Device → Screen
//!                                       ↕
//!                              [egui Overlay UI]
//! ```
//!
//! ## Frame Queue (Triple-Buffered)
//!
//! ```rust,ignore
//! struct FrameQueue {
//!     slots: [Frame; 3],
//!     display_index: AtomicU32,  // currently displayed
//!     decode_index: AtomicU32,  // being decoded
//!     write_index: AtomicU32,   // being written by network
//! }
//! ```
//!
//! Each slot is a `wgpu::Texture` that can be directly uploaded to.
//! The render thread polls `display_index` and composites the latest
//! available frame.

//! ## wgpu Compositor
//!
//! ```rust,ignore
//! struct ContinuumCompositor {
//!     device: wgpu::Device,
//!     queue: wgpu::Queue,
//!     pipeline: wgpu::RenderPipeline,
//!     textures: Vec<wgpu::Texture>,
//!     sampler: wgpu::Sampler,
//! }
//! ```
//!
//! The compositor renders a full-screen quad with the remote frame as
//! a texture. Keyboard/mouse input is captured via `winit::Window`
//! and forwarded to the network thread.

//! ## Fullscreen & Overlay
//!
//! - Exclusive fullscreen on the target monitor (via `winit`)
//! - egui renders an overlay on top of the compositor
//! - Auto-hide toolbar on mouse idle (2s timer)
//! - Stats overlay: FPS, latency, bandwidth (rendered as shader text)

//! ## Raw Input Capture
//!
//! ```rust,ignore
//! // Windows: RegisterRawInputDevices
//! // macOS: CGEventTap
//! // Linux: evdev / libinput
//!
//! struct RawInputCapture {
//!     mouse_delta: (f32, f32),     // relative movement
//!     absolute_pos: (u32, u32),    // absolute position
//!     buttons: u8,                 // bitmask of pressed buttons
//!     keys: Vec<VirtualKey>,       // pressed keys this frame
//! }
//! ```
//!
//! Raw input bypasses OS pointer acceleration and provides
//! sub-millisecond timestamping for precise input delivery.

pub struct GpuPipeline;
impl GpuPipeline {
    pub fn is_available() -> bool {
        cfg!(any(
            feature = "ffmpeg",
            target_os = "windows",
            target_os = "macos"
        ))
    }
}
