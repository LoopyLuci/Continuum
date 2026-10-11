//! CopyRect encoding (RFC 6143 7.7.2): the rectangle is a copy of another
//! area of the viewer's framebuffer, as it was before this rectangle.

use crate::codec::{DecodeError, PutBe, Reader};

pub fn encode(src_x: u16, src_y: u16, out: &mut Vec<u8>) {
    out.put_u16(src_x);
    out.put_u16(src_y);
}

/// Returns `((src_x, src_y), consumed)`.
pub fn decode(buf: &[u8]) -> Result<((u16, u16), usize), DecodeError> {
    let mut r = Reader::new(buf);
    Ok(((r.u16()?, r.u16()?), r.consumed()))
}
