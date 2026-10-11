//! Raw encoding (RFC 6143 7.7.1).

use super::PixelCodec;
use crate::codec::{DecodeError, Reader};
use crate::types::PixelFormat;

pub fn encode(pf: &PixelFormat, pixels: &[u32], out: &mut Vec<u8>) {
    let c = PixelCodec::new(pf);
    out.reserve(pixels.len() * c.pixel_len());
    for &p in pixels {
        c.put_pixel(p, out);
    }
}

/// Decodes a Raw payload; returns the pixel values and bytes consumed.
pub fn decode(pf: &PixelFormat, width: u16, height: u16, buf: &[u8]) -> Result<(Vec<u32>, usize), DecodeError> {
    let c = PixelCodec::new(pf);
    let n = usize::from(width) * usize::from(height);
    let mut r = Reader::new(buf);
    let data = r.bytes(n * c.pixel_len())?;
    Ok((data.chunks_exact(c.pixel_len()).map(|b| c.get_pixel(b)).collect(), r.consumed()))
}
