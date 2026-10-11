//! Tight encoding (rfbproto "Tight Encoding"): fill, and basic compression
//! with the copy or palette filter over four zlib streams. JPEG is not
//! produced (no pure-Rust encoder is wired in yet); the decoder refuses it.
//!
//! Rectangles must be at most [`MAX_WIDTH`] pixels wide (a limit existing
//! Tight decoders rely on); [`MAX_PIXELS`] is the area TightVNC itself
//! sends per rectangle, which keeps zlib bursts and decoder buffers small.
//! Use [`split`] to cut larger rectangles.

use super::palette::Palette;
use super::zlib::{Deflater, Inflater};
use super::PixelCodec;
use crate::codec::{DecodeError, Reader};
use crate::types::PixelFormat;

pub const MAX_WIDTH: u16 = 2048;
pub const MAX_PIXELS: usize = 65536;
/// Data shorter than this is sent without zlib.
pub const MIN_TO_COMPRESS: usize = 12;

const FILL: u8 = 0x80;
const JPEG: u8 = 0x90;
const EXPLICIT_FILTER: u8 = 0x40;
const FILTER_COPY: u8 = 0;
const FILTER_PALETTE: u8 = 1;
const FILTER_GRADIENT: u8 = 2;

const STREAM_FULL: u8 = 0;
const STREAM_MONO: u8 = 1;
const STREAM_INDEXED: u8 = 2;

/// Cuts `(x, y, w, h)` into pieces Tight can carry, row bands first.
pub fn split(x: u16, y: u16, w: u16, h: u16) -> Vec<(u16, u16, u16, u16)> {
    let mut out = Vec::new();
    let mut cx = 0;
    while cx < w {
        let cw = MAX_WIDTH.min(w - cx);
        let band = (MAX_PIXELS / usize::from(cw.max(1))).clamp(1, usize::from(u16::MAX)) as u16;
        let mut cy = 0;
        while cy < h {
            let ch = band.min(h - cy);
            out.push((x + cx, y + cy, cw, ch));
            cy += ch;
        }
        cx += cw;
    }
    out
}

fn put_compact_len(len: usize, out: &mut Vec<u8>) {
    debug_assert!(len < 1 << 22);
    let mut b = (len & 0x7f) as u8;
    if len > 0x7f {
        out.push(b | 0x80);
        b = ((len >> 7) & 0x7f) as u8;
        if len > 0x3fff {
            out.push(b | 0x80);
            b = ((len >> 14) & 0xff) as u8;
        }
    }
    out.push(b);
}

fn get_compact_len(r: &mut Reader<'_>) -> Result<usize, DecodeError> {
    let b0 = r.u8()?;
    let mut len = usize::from(b0 & 0x7f);
    if b0 & 0x80 != 0 {
        let b1 = r.u8()?;
        len |= usize::from(b1 & 0x7f) << 7;
        if b1 & 0x80 != 0 {
            len |= usize::from(r.u8()?) << 14;
        }
    }
    Ok(len)
}

/// The server side of a connection's four Tight zlib streams.
#[derive(Debug)]
pub struct TightEncoder {
    level: u8,
    streams: [Option<Deflater>; 4],
    data: Vec<u8>,
}

impl TightEncoder {
    /// `level`: zlib level 0-9 (the client's compression-level hint).
    pub fn new(level: u8) -> Self {
        TightEncoder { level: level.min(9), streams: [None, None, None, None], data: Vec::new() }
    }

    fn finish(&mut self, stream: u8, out: &mut Vec<u8>) {
        if self.data.len() < MIN_TO_COMPRESS {
            out.extend_from_slice(&self.data);
            return;
        }
        let level = self.level;
        let z = self.streams[usize::from(stream)].get_or_insert_with(|| Deflater::new(level));
        let mut packed = Vec::with_capacity(self.data.len() / 2 + 64);
        z.compress(&self.data, &mut packed);
        put_compact_len(packed.len(), out);
        out.extend_from_slice(&packed);
    }

    /// Appends the Tight payload for one rectangle of pixel values in `pf`
    /// (at most [`MAX_WIDTH`] wide; see [`split`]).
    pub fn encode(&mut self, pf: &PixelFormat, width: u16, height: u16, pixels: &[u32], out: &mut Vec<u8>) {
        let (w, h) = (usize::from(width), usize::from(height));
        assert_eq!(pixels.len(), w * h, "pixel count does not match the rectangle");
        debug_assert!(width <= MAX_WIDTH, "Tight rectangles must be split to {MAX_WIDTH} pixels wide");
        let c = PixelCodec::new(pf);
        let tp = c.tpixel_len();
        let mut pal = Palette::new(256);
        for &v in pixels {
            if pal.insert(v).is_none() {
                break;
            }
        }
        let n = pal.len();
        self.data.clear();
        if n <= 1 {
            out.push(FILL);
            c.put_tpixel(pixels.first().copied().unwrap_or(0), out);
            return;
        }
        // A palette pays off when its indices plus the colour table are
        // clearly smaller than full-colour pixels.
        if !pal.overflowed() && (n == 2 || n * tp * 2 <= w * h) {
            let stream = if n == 2 { STREAM_MONO } else { STREAM_INDEXED };
            out.push((stream << 4) | EXPLICIT_FILTER);
            out.push(FILTER_PALETTE);
            out.push((n - 1) as u8);
            pal.colours.iter().for_each(|&v| c.put_tpixel(v, out));
            if n == 2 {
                for row in pixels.chunks_exact(w) {
                    for bits in row.chunks(8) {
                        let mut b = 0u8;
                        for (i, &v) in bits.iter().enumerate() {
                            b |= pal.get(v) << (7 - i);
                        }
                        self.data.push(b);
                    }
                }
            } else {
                self.data.extend(pixels.iter().map(|&v| pal.get(v)));
            }
            return self.finish(stream, out);
        }
        out.push(STREAM_FULL << 4); // basic, copy filter implied
        self.data.reserve(w * h * tp);
        let mut data = std::mem::take(&mut self.data);
        pixels.iter().for_each(|&v| c.put_tpixel(v, &mut data));
        self.data = data;
        self.finish(STREAM_FULL, out)
    }
}

/// The client side of a connection's Tight streams (a reference decoder).
#[derive(Debug, Default)]
pub struct TightDecoder {
    streams: [Option<Inflater>; 4],
}

impl TightDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one Tight payload. On `Incomplete` nothing is consumed and
    /// no stream state changes.
    pub fn decode(
        &mut self,
        pf: &PixelFormat,
        width: u16,
        height: u16,
        buf: &[u8],
    ) -> Result<(Vec<u32>, usize), DecodeError> {
        let (w, h) = (usize::from(width), usize::from(height));
        let n = w * h;
        let c = PixelCodec::new(pf);
        let tp = c.tpixel_len();
        let mut r = Reader::new(buf);
        let ctl = r.u8()?;
        let resets = ctl & 0x0f;
        let comp = ctl & 0xf0;
        if comp == FILL {
            let v = c.get_tpixel(r.bytes(tp)?);
            self.reset(resets);
            return Ok((vec![v; n], r.consumed()));
        }
        if comp == JPEG {
            return Err(DecodeError::Invalid("Tight JPEG is not supported by this decoder".into()));
        }
        if comp & 0x80 != 0 {
            return Err(DecodeError::Invalid(format!("Tight compression type {:#x} is not defined", comp >> 4)));
        }
        let stream = usize::from((comp >> 4) & 3);
        let filter = if comp & EXPLICIT_FILTER != 0 { r.u8()? } else { FILTER_COPY };
        let mut palette = Vec::new();
        let len = match filter {
            FILTER_COPY => n * tp,
            FILTER_PALETTE => {
                let k = usize::from(r.u8()?) + 1;
                palette = r.bytes(k * tp)?.chunks_exact(tp).map(|b| c.get_tpixel(b)).collect();
                if k <= 2 {
                    h * w.div_ceil(8)
                } else {
                    n
                }
            }
            FILTER_GRADIENT => {
                if tp != 3 {
                    return Err(DecodeError::Invalid("Tight gradient filter needs 24-bit TPIXELs".into()));
                }
                n * 3
            }
            f => return Err(DecodeError::Invalid(format!("Tight filter {f} is not defined"))),
        };
        let data: Vec<u8> = if len < MIN_TO_COMPRESS {
            let d = r.bytes(len)?.to_vec();
            self.reset(resets);
            d
        } else {
            let clen = get_compact_len(&mut r)?;
            let packed = r.bytes(clen)?;
            self.reset(resets);
            let z = self.streams[stream].get_or_insert_with(Inflater::new);
            z.inflate(packed, len)?
        };
        if data.len() != len {
            return Err(DecodeError::Invalid(format!("Tight data is {} bytes, expected {len}", data.len())));
        }
        let px = match filter {
            FILTER_COPY => data.chunks_exact(tp).map(|b| c.get_tpixel(b)).collect(),
            FILTER_PALETTE => {
                let pick = |i: usize| {
                    palette
                        .get(i)
                        .copied()
                        .ok_or_else(|| DecodeError::Invalid("Tight palette index out of range".into()))
                };
                let mut px = Vec::with_capacity(n);
                if palette.len() <= 2 {
                    for row in data.chunks_exact(w.div_ceil(8)) {
                        for x in 0..w {
                            px.push(pick(usize::from((row[x / 8] >> (7 - x % 8)) & 1))?);
                        }
                    }
                } else {
                    for &i in &data {
                        px.push(pick(usize::from(i))?);
                    }
                }
                px
            }
            _ => {
                // Gradient: each channel is predicted as left + above - above-left.
                let mut rgb = vec![0u8; n * 3];
                for y in 0..h {
                    for x in 0..w {
                        for ch in 0..3 {
                            let at = |xx: usize, yy: usize| i32::from(rgb[(yy * w + xx) * 3 + ch]);
                            let left = if x > 0 { at(x - 1, y) } else { 0 };
                            let up = if y > 0 { at(x, y - 1) } else { 0 };
                            let ul = if x > 0 && y > 0 { at(x - 1, y - 1) } else { 0 };
                            let pred = (left + up - ul).clamp(0, 255) as u8;
                            let i = (y * w + x) * 3 + ch;
                            rgb[i] = pred.wrapping_add(data[i]);
                        }
                    }
                }
                rgb.chunks_exact(3).map(|b| c.get_tpixel(b)).collect()
            }
        };
        Ok((px, r.consumed()))
    }

    fn reset(&mut self, mask: u8) {
        for (i, s) in self.streams.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *s = None;
            }
        }
    }
}
