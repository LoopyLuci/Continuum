//! Rectangle encodings: pure encoders for the host and reference decoders
//! (used by tests, fuzzing, and any client that wants them).
//!
//! Everything works on pixel *values* in the client's
//! [`PixelFormat`](crate::PixelFormat) (the number a PIXEL encodes, e.g.
//! `r << red_shift | g << green_shift | b << blue_shift`), row-major, so
//! `decode(encode(px)) == px`. Payloads are what follows the 12-byte
//! [`RectHeader`](crate::RectHeader). Callers must check a rectangle fits
//! the framebuffer before decoding it.
//!
//! ZRLE and Tight keep zlib state for the whole connection: use one encoder
//! (and one decoder) per connection, and feed every rectangle through it in
//! order.
//!
//! ```
//! use rfb_proto::encodings::{ZrleDecoder, ZrleEncoder};
//! use rfb_proto::PixelFormat;
//!
//! let pf = PixelFormat::RGBX8888_LE;
//! let pixels: Vec<u32> = (0..64 * 32).map(|i| if i % 64 < 32 { 0xff0000 } else { 0x0000ff }).collect();
//! let mut payload = Vec::new();
//! ZrleEncoder::new(6).encode(&pf, 64, 32, &pixels, &mut payload);
//! let (decoded, used) = ZrleDecoder::new().decode(&pf, 64, 32, &payload).unwrap();
//! assert_eq!((decoded, used), (pixels, payload.len()));
//! ```
//!
//! CPIXEL (ZRLE) follows RFC 6143 7.7.6 exactly: for 32-bit true colour
//! with depth <= 24 it is 3 bytes, namely the least significant three when
//! every colour bit is there, or the most significant three when every
//! colour bit is there instead (colours in the top byte; libvncserver gets
//! this case wrong). Otherwise it is the full PIXEL. See [`PixelCodec`].

pub mod classic;
pub mod copyrect;
mod palette;
pub mod pixel;
pub mod raw;
pub mod tight;
pub mod zlib;
pub mod zrle;

pub use pixel::PixelCodec;
pub use tight::{TightDecoder, TightEncoder};
pub use zrle::{ZrleDecoder, ZrleEncoder};

use crate::types::Encoding;

/// The zlib level a client asked for with the CompressLevel pseudo-encodings
/// (-256 = level 0 ... -247 = level 9); the last one in the list wins.
pub fn compress_level(encodings: &[Encoding]) -> Option<u8> {
    encodings.iter().rev().find_map(|e| match e.code() {
        c @ -256..=-247 => Some((c + 256) as u8),
        _ => None,
    })
}
