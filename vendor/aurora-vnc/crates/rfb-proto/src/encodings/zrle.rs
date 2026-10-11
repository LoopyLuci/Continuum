//! ZRLE encoding (RFC 6143 7.7.6): 64x64 tiles, each raw, solid, packed
//! palette, plain RLE or palette RLE (whichever is smallest), all through
//! one zlib stream per connection.

use super::palette::Palette;
use super::zlib::{Deflater, Inflater};
use super::PixelCodec;
use crate::codec::{DecodeError, Reader};
use crate::types::PixelFormat;

pub const TILE: usize = 64;

/// Bytes needed to send a run length with ZRLE's 255-continuation scheme.
fn run_len_bytes(len: usize) -> usize {
    (len - 1) / 255 + 1
}

fn put_run_len(mut len: usize, out: &mut Vec<u8>) {
    len -= 1;
    while len >= 255 {
        out.push(255);
        len -= 255;
    }
    out.push(len as u8);
}

/// Appends the uncompressed encoding of one tile (row-major `px`, `w` wide).
fn encode_tile(c: &PixelCodec, px: &[u32], w: usize, out: &mut Vec<u8>) {
    let h = px.len() / w;
    let cp = c.cpixel_len();
    let mut pal = Palette::new(127);
    let mut plain_rle = 0usize;
    let mut pal_rle = 0usize;
    let mut i = 0;
    while i < px.len() {
        let v = px[i];
        let mut j = i + 1;
        while j < px.len() && px[j] == v {
            j += 1;
        }
        let len = j - i;
        plain_rle += cp + run_len_bytes(len);
        pal_rle += if len == 1 { 1 } else { 1 + run_len_bytes(len) };
        pal.insert(v);
        i = j;
    }
    let n = pal.len();
    if n == 1 {
        out.push(1);
        c.put_cpixel(px[0], out);
        return;
    }
    // Candidates in a fixed order; the first smallest wins.
    let raw = px.len() * cp;
    let mut best = (raw, 0u8);
    if !pal.overflowed() {
        if n <= 16 {
            let bits = match n {
                2 => 1,
                3..=4 => 2,
                _ => 4,
            };
            let packed = n * cp + h * (w * bits).div_ceil(8);
            if packed < best.0 {
                best = (packed, n as u8);
            }
        }
        let prle = n * cp + pal_rle;
        if prle < best.0 {
            best = (prle, 128 + n as u8);
        }
    }
    if plain_rle < best.0 {
        best = (plain_rle, 128);
    }
    let sub = best.1;
    out.push(sub);
    match sub {
        0 => px.iter().for_each(|&v| c.put_cpixel(v, out)),
        2..=16 => {
            pal.colours.iter().for_each(|&v| c.put_cpixel(v, out));
            let bits = match sub {
                2 => 1,
                3..=4 => 2,
                _ => 4,
            };
            for row in px.chunks_exact(w) {
                let mut byte = 0u8;
                let mut used = 0;
                for &v in row {
                    byte |= pal.get(v) << (8 - bits - used);
                    used += bits;
                    if used == 8 {
                        out.push(byte);
                        byte = 0;
                        used = 0;
                    }
                }
                if used > 0 {
                    out.push(byte);
                }
            }
        }
        _ => {
            if sub > 128 {
                pal.colours.iter().for_each(|&v| c.put_cpixel(v, out));
            }
            let mut i = 0;
            while i < px.len() {
                let v = px[i];
                let mut j = i + 1;
                while j < px.len() && px[j] == v {
                    j += 1;
                }
                let len = j - i;
                if sub == 128 {
                    c.put_cpixel(v, out);
                    put_run_len(len, out);
                } else if len == 1 {
                    out.push(pal.get(v));
                } else {
                    out.push(pal.get(v) | 0x80);
                    put_run_len(len, out);
                }
                i = j;
            }
        }
    }
}

/// The server side of a connection's ZRLE stream.
#[derive(Debug)]
pub struct ZrleEncoder {
    z: Deflater,
    tiles: Vec<u8>,
    tile: Vec<u32>,
}

impl ZrleEncoder {
    /// `level`: zlib level 0-9 (the client's compression-level hint).
    pub fn new(level: u8) -> Self {
        ZrleEncoder { z: Deflater::new(level), tiles: Vec::new(), tile: Vec::with_capacity(TILE * TILE) }
    }

    /// Appends the ZRLE payload (length + zlib data) for a `width` x
    /// `height` rectangle of pixel values in `pf`, row-major.
    pub fn encode(&mut self, pf: &PixelFormat, width: u16, height: u16, pixels: &[u32], out: &mut Vec<u8>) {
        let (w, h) = (usize::from(width), usize::from(height));
        assert_eq!(pixels.len(), w * h, "pixel count does not match the rectangle");
        let c = PixelCodec::new(pf);
        self.tiles.clear();
        for ty in (0..h).step_by(TILE) {
            let th = TILE.min(h - ty);
            for tx in (0..w).step_by(TILE) {
                let tw = TILE.min(w - tx);
                self.tile.clear();
                for y in ty..ty + th {
                    self.tile.extend_from_slice(&pixels[y * w + tx..y * w + tx + tw]);
                }
                encode_tile(&c, &self.tile, tw, &mut self.tiles);
            }
        }
        let at = out.len();
        out.extend_from_slice(&[0; 4]);
        self.z.compress(&self.tiles, out);
        let len = (out.len() - at - 4) as u32;
        out[at..at + 4].copy_from_slice(&len.to_be_bytes());
    }
}

/// The client side of a connection's ZRLE stream (a reference decoder).
#[derive(Debug, Default)]
pub struct ZrleDecoder {
    z: Inflater,
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos + n;
        let s = self.b.get(self.pos..end).ok_or_else(|| DecodeError::Invalid("ZRLE tile data ends early".into()))?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn run_len(&mut self, left: usize) -> Result<usize, DecodeError> {
        let mut len = 1usize;
        loop {
            let b = self.u8()?;
            len += usize::from(b);
            if len > left {
                return Err(DecodeError::Invalid("ZRLE run past the end of its tile".into()));
            }
            if b != 255 {
                return Ok(len);
            }
        }
    }
}

impl ZrleDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one ZRLE payload. On `Incomplete` nothing is consumed and the
    /// stream state is untouched.
    pub fn decode(
        &mut self,
        pf: &PixelFormat,
        width: u16,
        height: u16,
        buf: &[u8],
    ) -> Result<(Vec<u32>, usize), DecodeError> {
        let (w, h) = (usize::from(width), usize::from(height));
        let c = PixelCodec::new(pf);
        let cp = c.cpixel_len();
        let tiles = w.div_ceil(TILE) * h.div_ceil(TILE);
        // Largest possible tile stream: palette RLE of single pixels costs
        // 2 bytes a pixel, plain RLE cp + 1, plus each tile's header/palette.
        let bound = tiles * (1 + 127 * cp) + w * h * (cp + 2);
        let mut r = Reader::new(buf);
        let data = r.len_prefixed(u32::try_from(bound + 1024).unwrap_or(u32::MAX), "ZRLE data")?;
        let consumed = r.consumed();
        let raw = self.z.inflate(data, bound)?;
        let mut cur = Cursor { b: &raw, pos: 0 };
        let mut px = vec![0u32; w * h];
        for ty in (0..h).step_by(TILE) {
            let th = TILE.min(h - ty);
            for tx in (0..w).step_by(TILE) {
                let tw = TILE.min(w - tx);
                let n = tw * th;
                let mut tile = Vec::with_capacity(n);
                let sub = cur.u8()?;
                let read_pal = |cur: &mut Cursor<'_>, k: usize| -> Result<Vec<u32>, DecodeError> {
                    Ok(cur.take(k * cp)?.chunks_exact(cp).map(|b| c.get_cpixel(b)).collect())
                };
                match sub {
                    0 => tile.extend(cur.take(n * cp)?.chunks_exact(cp).map(|b| c.get_cpixel(b))),
                    1 => {
                        let v = c.get_cpixel(cur.take(cp)?);
                        tile.resize(n, v);
                    }
                    2..=16 => {
                        let pal = read_pal(&mut cur, usize::from(sub))?;
                        let bits = match sub {
                            2 => 1,
                            3..=4 => 2,
                            _ => 4,
                        };
                        let row_bytes = (tw * bits).div_ceil(8);
                        for _ in 0..th {
                            let row = cur.take(row_bytes)?;
                            for x in 0..tw {
                                let bit = x * bits;
                                let i = (row[bit / 8] >> (8 - bits - bit % 8)) & ((1 << bits) - 1);
                                let v = *pal
                                    .get(usize::from(i))
                                    .ok_or_else(|| DecodeError::Invalid("ZRLE palette index out of range".into()))?;
                                tile.push(v);
                            }
                        }
                    }
                    128 => {
                        while tile.len() < n {
                            let v = c.get_cpixel(cur.take(cp)?);
                            let len = cur.run_len(n - tile.len())?;
                            tile.resize(tile.len() + len, v);
                        }
                    }
                    130..=255 => {
                        let pal = read_pal(&mut cur, usize::from(sub - 128))?;
                        while tile.len() < n {
                            let b = cur.u8()?;
                            let v = *pal
                                .get(usize::from(b & 0x7f))
                                .ok_or_else(|| DecodeError::Invalid("ZRLE palette index out of range".into()))?;
                            let len = if b & 0x80 != 0 { cur.run_len(n - tile.len())? } else { 1 };
                            tile.resize(tile.len() + len, v);
                        }
                    }
                    s => return Err(DecodeError::Invalid(format!("ZRLE subencoding {s} is not defined"))),
                }
                for (row, chunk) in tile.chunks_exact(tw).enumerate() {
                    let at = (ty + row) * w + tx;
                    px[at..at + tw].copy_from_slice(chunk);
                }
            }
        }
        if cur.pos != raw.len() {
            return Err(DecodeError::Invalid("ZRLE data continues past the last tile".into()));
        }
        Ok((px, consumed))
    }
}
