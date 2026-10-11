//! Persistent zlib streams (pure Rust, miniz_oxide). ZRLE uses one stream
//! per connection and Tight four; every rectangle ends with a sync flush so
//! the peer can decode it without waiting for more data.

use miniz_oxide::deflate::core::CompressorOxide;
use miniz_oxide::inflate::stream::InflateState;
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

use crate::codec::DecodeError;

/// The compressing end of one zlib stream.
pub struct Deflater {
    c: Box<CompressorOxide>,
    level: u8,
}

impl std::fmt::Debug for Deflater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deflater").field("level", &self.level).finish_non_exhaustive()
    }
}

impl Deflater {
    /// `level` is the zlib level, 0 (stored, no compression) to 9.
    pub fn new(level: u8) -> Self {
        let level = level.min(9);
        let mut c = Box::<CompressorOxide>::default();
        c.set_format_and_level(DataFormat::Zlib, level);
        Deflater { c, level }
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    /// Compresses `input` and appends it, sync-flushed, to `out`.
    pub fn compress(&mut self, input: &[u8], out: &mut Vec<u8>) {
        const CHUNK: usize = 64 * 1024;
        let mut pos = 0;
        loop {
            let start = out.len();
            out.resize(start + CHUNK, 0);
            let r = miniz_oxide::deflate::stream::deflate(&mut self.c, &input[pos..], &mut out[start..], MZFlush::Sync);
            out.truncate(start + r.bytes_written);
            pos += r.bytes_consumed;
            match r.status {
                Ok(_) => {}
                // No progress possible: everything is already flushed.
                Err(MZError::Buf) => break,
                Err(e) => unreachable!("deflate failed: {e:?}"),
            }
            if pos >= input.len() && r.bytes_written < CHUNK {
                break;
            }
        }
    }
}

/// The decompressing end of one zlib stream.
pub struct Inflater {
    s: Box<InflateState>,
}

impl std::fmt::Debug for Inflater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inflater").finish_non_exhaustive()
    }
}

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Inflater {
    pub fn new() -> Self {
        Inflater { s: InflateState::new_boxed(DataFormat::Zlib) }
    }

    /// Decompresses all of `input`, refusing to produce more than `max_out`
    /// bytes. The stream state carries over to the next call.
    pub fn inflate(&mut self, input: &[u8], max_out: usize) -> Result<Vec<u8>, DecodeError> {
        const CHUNK: usize = 32 * 1024;
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            let start = out.len();
            let room = CHUNK.min(max_out - start + 1);
            out.resize(start + room, 0);
            let r = miniz_oxide::inflate::stream::inflate(&mut self.s, &input[pos..], &mut out[start..], MZFlush::None);
            out.truncate(start + r.bytes_written);
            pos += r.bytes_consumed;
            if out.len() > max_out {
                return Err(DecodeError::Invalid(format!("zlib data inflates past {max_out} bytes")));
            }
            match r.status {
                Ok(MZStatus::StreamEnd) => break,
                Ok(_) => {}
                Err(MZError::Buf) => break, // needs more input than this rectangle has
                Err(e) => return Err(DecodeError::Invalid(format!("corrupt zlib data ({e:?})"))),
            }
            if pos >= input.len() && r.bytes_written < room {
                break;
            }
            if r.bytes_consumed == 0 && r.bytes_written == 0 {
                break;
            }
        }
        if pos < input.len() {
            return Err(DecodeError::Invalid("zlib data continues past the end of its stream".into()));
        }
        Ok(out)
    }
}
