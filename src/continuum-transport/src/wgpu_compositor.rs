#![allow(unexpected_cfgs)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct FrameBuffer {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    pub timestamp_us: i64,
    pub frame_number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Rgba,
    Nv12,
    Bgra,
}

pub struct TripleBufferQueue {
    buffers: [FrameBuffer; 3],
    write_idx: AtomicU32,
    display_idx: AtomicU32,
    decode_idx: AtomicU32,
}

impl TripleBufferQueue {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;
        Self {
            buffers: [
                FrameBuffer {
                    data: vec![0u8; size],
                    width,
                    height,
                    format: FrameFormat::Rgba,
                    timestamp_us: 0,
                    frame_number: 0,
                },
                FrameBuffer {
                    data: vec![0u8; size],
                    width,
                    height,
                    format: FrameFormat::Rgba,
                    timestamp_us: 0,
                    frame_number: 0,
                },
                FrameBuffer {
                    data: vec![0u8; size],
                    width,
                    height,
                    format: FrameFormat::Rgba,
                    timestamp_us: 0,
                    frame_number: 0,
                },
            ],
            write_idx: AtomicU32::new(0),
            display_idx: AtomicU32::new(0),
            decode_idx: AtomicU32::new(1),
        }
    }

    pub fn acquire_write(&self) -> u32 {
        let w = self.write_idx.load(Ordering::Acquire);
        let d = self.display_idx.load(Ordering::Acquire);
        let mut candidate = w;
        loop {
            candidate = (candidate + 1) % 3;
            if candidate != d {
                break;
            }
            candidate = (candidate + 1) % 3;
        }
        self.write_idx.store(candidate, Ordering::Release);
        candidate
    }

    pub fn acquire_decode(&self) -> u32 {
        let w = self.write_idx.load(Ordering::Acquire);
        self.decode_idx.store(w, Ordering::Release);
        w
    }

    pub fn acquire_display(&self) -> u32 {
        let d = self.decode_idx.load(Ordering::Acquire);
        self.display_idx.store(d, Ordering::Release);
        d
    }

    pub fn get_buffer(&self, idx: u32) -> &FrameBuffer {
        &self.buffers[idx as usize]
    }

    pub fn get_buffer_mut(&mut self, idx: u32) -> &mut FrameBuffer {
        &mut self.buffers[idx as usize]
    }
}

pub struct WgpuCompositor;

impl WgpuCompositor {
    pub fn is_supported() -> bool {
        cfg!(all(
            any(
                target_os = "windows",
                target_os = "macos",
                target_os = "linux"
            ),
            feature = "wgpu"
        ))
    }

    pub fn build_shader_source() -> &'static str {
        r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let verts = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let uvs = array<vec2<f32>, 6>(
        vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(1.0, 0.0),
    );
    return VertexOutput(verts[vi], uvs[vi]);
}

@group(0) @binding(0) var remote_tex: texture_2d<f32>;
@group(0) @binding(1) var sampler_: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(remote_tex, sampler_, in.uv);
}
"#
    }
}

pub struct GpuPipeline {
    pub triple_buffer: TripleBufferQueue,
    pub queue: Arc<FrameQueueStats>,
}

pub struct FrameQueueStats {
    pub frames_queued: AtomicU32,
    pub frames_consumed: AtomicU32,
    pub dropped_frames: AtomicU32,
}

impl GpuPipeline {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            triple_buffer: TripleBufferQueue::new(width, height),
            queue: Arc::new(FrameQueueStats {
                frames_queued: AtomicU32::new(0),
                frames_consumed: AtomicU32::new(0),
                dropped_frames: AtomicU32::new(0),
            }),
        }
    }
}
