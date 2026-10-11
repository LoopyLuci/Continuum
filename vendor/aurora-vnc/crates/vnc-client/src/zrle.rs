//! ZRLE decoding (RFC 6143 §7.7.6).
//!
//! One zlib stream spans the whole connection, so a [`ZrleDecoder`] lives as
//! long as the session. Every length and index is checked: a hostile server
//! can make decoding fail, never read out of bounds or inflate without limit.

use flate2::{Decompress, FlushDecompress, Status};
use rfb_proto::PixelFormat;

use crate::framebuffer::{read_pixel, to_rgb, Framebuffer, Rect};

const TILE: u16 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZrleError(pub String);

impl std::fmt::Display for ZrleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ZRLE: {}", self.0)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, ZrleError> {
    Err(ZrleError(m.into()))
}

pub struct ZrleDecoder {
    z: Decompress,
    scratch: Vec<u8>,
    tile: Vec<u32>,
}

impl Default for ZrleDecoder {
    fn default() -> Self {
        ZrleDecoder { z: Decompress::new(true), scratch: Vec::new(), tile: Vec::with_capacity(64 * 64) }
    }
}

/// How CPIXELs are laid out for a pixel format.
#[derive(Clone, Copy)]
enum CPixel {
    /// Same as a full pixel.
    Full(usize),
    /// 3 bytes holding the least significant 24 bits of a 32-bit pixel.
    Ls3,
    /// 3 bytes holding the most significant 24 bits of a 32-bit pixel.
    Ms3,
}

impl CPixel {
    fn of(pf: &PixelFormat) -> Self {
        if pf.true_colour && pf.bits_per_pixel == 32 && pf.depth <= 24 {
            let mask = |max: u16, shift: u8| (max as u64) << shift;
            let all =
                mask(pf.red_max, pf.red_shift) | mask(pf.green_max, pf.green_shift) | mask(pf.blue_max, pf.blue_shift);
            if all <= 0x00ff_ffff {
                return CPixel::Ls3;
            }
            if all & 0xff == 0 && all <= 0xffff_ffff {
                return CPixel::Ms3;
            }
        }
        CPixel::Full(pf.bytes_per_pixel())
    }

    fn len(self) -> usize {
        match self {
            CPixel::Full(n) => n,
            CPixel::Ls3 | CPixel::Ms3 => 3,
        }
    }
}

/// Inflates all of `input` (one rectangle's share of a connection-long zlib
/// stream, sync-flushed by the server) into `out`, refusing to produce more
/// than `limit` bytes.
pub(crate) fn inflate(z: &mut Decompress, mut input: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), ZrleError> {
    out.clear();
    let zerr = |e: flate2::DecompressError| ZrleError(format!("zlib: {e}"));
    while !input.is_empty() {
        if out.len() >= limit {
            return err("decompressed data exceeds the rectangle's maximum size");
        }
        out.reserve((limit - out.len()).clamp(1, 256 * 1024));
        let before_in = z.total_in();
        let before_out = out.len();
        let status = z.decompress_vec(input, out, FlushDecompress::Sync).map_err(zerr)?;
        let used = (z.total_in() - before_in) as usize;
        input = &input[used..];
        if matches!(status, Status::StreamEnd) && !input.is_empty() {
            return err("zlib stream ended early");
        }
        // A call may only copy out window bytes held back when the output
        // buffer filled; that reads no input but is still progress.
        if used == 0 && out.len() == before_out {
            return err("zlib made no progress");
        }
    }
    // Drain output still buffered inside the decompressor.
    loop {
        let before = out.len();
        if before >= limit {
            break;
        }
        out.reserve((limit - before).min(64 * 1024));
        z.decompress_vec(&[], out, FlushDecompress::Sync).map_err(zerr)?;
        if out.len() == before {
            break;
        }
    }
    if out.len() > limit {
        return err("decompressed data exceeds the rectangle's maximum size");
    }
    Ok(())
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ZrleError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else { return err("tile data truncated") };
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ZrleError> {
        Ok(self.take(1)?[0])
    }

    /// Run length: bytes summed while 255, plus one.
    fn run_len(&mut self, max: usize) -> Result<usize, ZrleError> {
        let mut n = 1usize;
        loop {
            let b = self.u8()?;
            n += b as usize;
            if n > max {
                return err("run longer than tile");
            }
            if b != 255 {
                return Ok(n);
            }
        }
    }
}

impl ZrleDecoder {
    /// Upper bound on decompressed bytes for a rect, so a small compressed
    /// payload cannot balloon (worst case is plain RLE of single pixels).
    fn max_inflated(r: Rect, cp: usize) -> usize {
        let tiles = (r.width as usize).div_ceil(TILE as usize) * (r.height as usize).div_ceil(TILE as usize);
        r.area() as usize * (cp + 1) + tiles * (1 + 127 * cp) + 64
    }

    /// Decodes one ZRLE rectangle payload (after the 4-byte length) into `fb`.
    /// Caller guarantees `r` fits in `fb`.
    pub fn decode(
        &mut self,
        r: Rect,
        compressed: &[u8],
        pf: &PixelFormat,
        colour_map: &[u32],
        fb: &mut Framebuffer,
    ) -> Result<(), ZrleError> {
        let cp = CPixel::of(pf);
        inflate(&mut self.z, compressed, &mut self.scratch, Self::max_inflated(r, cp.len()))?;
        let data = std::mem::take(&mut self.scratch);
        let res = self.decode_tiles(r, &data, cp, pf, colour_map, fb);
        self.scratch = data;
        res
    }

    fn decode_tiles(
        &mut self,
        r: Rect,
        data: &[u8],
        cp: CPixel,
        pf: &PixelFormat,
        cmap: &[u32],
        fb: &mut Framebuffer,
    ) -> Result<(), ZrleError> {
        let mut rd = Reader { b: data, pos: 0 };
        let read_cpixel = |rd: &mut Reader| -> Result<u32, ZrleError> {
            let b = rd.take(cp.len())?;
            let v = match cp {
                CPixel::Full(_) => read_pixel(b, pf),
                CPixel::Ls3 | CPixel::Ms3 => {
                    let v = if pf.big_endian {
                        u32::from_be_bytes([0, b[0], b[1], b[2]])
                    } else {
                        u32::from_le_bytes([b[0], b[1], b[2], 0])
                    };
                    if matches!(cp, CPixel::Ms3) {
                        v << 8
                    } else {
                        v
                    }
                }
            };
            Ok(to_rgb(v, pf, cmap))
        };
        let mut ty = 0;
        while ty < r.height {
            let th = TILE.min(r.height - ty);
            let mut tx = 0;
            while tx < r.width {
                let tw = TILE.min(r.width - tx);
                let tr = Rect { x: r.x + tx, y: r.y + ty, width: tw, height: th };
                let n = tw as usize * th as usize;
                let sub = rd.u8()?;
                match sub {
                    0 => {
                        self.tile.clear();
                        for _ in 0..n {
                            self.tile.push(read_cpixel(&mut rd)?);
                        }
                        fb.put_rgb(tr, &self.tile);
                    }
                    1 => fb.fill(tr, read_cpixel(&mut rd)?),
                    2..=16 => {
                        let mut pal = [0u32; 16];
                        for p in pal.iter_mut().take(sub as usize) {
                            *p = read_cpixel(&mut rd)?;
                        }
                        let bits = match sub {
                            2 => 1,
                            3..=4 => 2,
                            _ => 4,
                        };
                        let row_bytes = (tw as usize * bits).div_ceil(8);
                        self.tile.clear();
                        for _ in 0..th {
                            let row = rd.take(row_bytes)?;
                            for x in 0..tw as usize {
                                let bit = x * bits;
                                let idx = (row[bit / 8] >> (8 - bits - bit % 8)) & ((1 << bits) - 1);
                                if idx >= sub {
                                    return err("palette index out of range");
                                }
                                self.tile.push(pal[idx as usize]);
                            }
                        }
                        fb.put_rgb(tr, &self.tile);
                    }
                    128 => {
                        self.tile.clear();
                        while self.tile.len() < n {
                            let px = read_cpixel(&mut rd)?;
                            let len = rd.run_len(n - self.tile.len())?;
                            self.tile.extend(std::iter::repeat_n(px, len));
                        }
                        fb.put_rgb(tr, &self.tile);
                    }
                    130..=255 => {
                        let size = (sub - 128) as usize;
                        let mut pal = [0u32; 127];
                        for p in pal.iter_mut().take(size) {
                            *p = read_cpixel(&mut rd)?;
                        }
                        self.tile.clear();
                        while self.tile.len() < n {
                            let b = rd.u8()?;
                            let idx = (b & 0x7f) as usize;
                            if idx >= size {
                                return err("palette index out of range");
                            }
                            let len = if b & 0x80 != 0 { rd.run_len(n - self.tile.len())? } else { 1 };
                            self.tile.extend(std::iter::repeat_n(pal[idx], len));
                        }
                        fb.put_rgb(tr, &self.tile);
                    }
                    other => return err(format!("invalid subencoding {other}")),
                }
                tx += tw;
            }
            ty += th;
        }
        if rd.pos != data.len() {
            return err(format!("{} unused bytes after the last tile", data.len() - rd.pos));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    /// A test-side encoder sharing one zlib stream across rects, like a server.
    struct Enc(Compress);

    impl Enc {
        fn new() -> Self {
            Enc(Compress::new(Compression::default(), true))
        }
        fn rect(&mut self, raw: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(raw.len() + 64);
            self.0.compress_vec(raw, &mut out, FlushCompress::Sync).unwrap();
            while out.len() == out.capacity() {
                out.reserve(1024);
                self.0.compress_vec(&[], &mut out, FlushCompress::Sync).unwrap();
            }
            out
        }
    }

    const PF: PixelFormat = PixelFormat::RGBX8888_LE;

    fn cpx(rgb: u32) -> [u8; 3] {
        [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8]
    }

    fn decode(d: &mut ZrleDecoder, r: Rect, z: &[u8], fb: &mut Framebuffer) -> Result<(), ZrleError> {
        d.decode(r, z, &PF, &[], fb)
    }

    #[test]
    fn all_subencodings_across_tiles_with_shared_stream() {
        // 70x2 = two tiles (64 wide + 6 wide) per rect.
        let r = Rect { x: 0, y: 0, width: 70, height: 2 };
        let mut fb = Framebuffer::new(70, 2);
        let mut d = ZrleDecoder::default();
        let mut enc = Enc::new();

        // Rect 1: tile A solid, tile B raw.
        let mut t = vec![1];
        t.extend(cpx(0x112233));
        t.push(0);
        for i in 0..12u32 {
            t.extend(cpx(i));
        }
        decode(&mut d, r, &enc.rect(&t), &mut fb).unwrap();
        assert_eq!(fb.pixel(10, 1), 0x112233);
        assert_eq!(fb.pixel(64 + 5, 1), 11);

        // Rect 2 (same zlib stream): tile A plain RLE, tile B packed palette (2 colours, 1 bit).
        let mut t = vec![128];
        t.extend(cpx(0xaa0000));
        t.push(99); // 100 pixels
        t.extend(cpx(0x00bb00));
        t.push(27); // 28 more = 128
        t.push(2);
        t.extend(cpx(0x000001));
        t.extend(cpx(0x000002));
        t.extend([0b1010_1000, 0b0101_0100]); // rows of 6 px, MSB first
        decode(&mut d, r, &enc.rect(&t), &mut fb).unwrap();
        // Tile A is 64x2: pixels 0..=99 red (row 1 up to x=35), then green.
        assert_eq!(fb.pixel(63, 0), 0xaa0000);
        assert_eq!(fb.pixel(35, 1), 0xaa0000);
        assert_eq!(fb.pixel(36, 1), 0x00bb00);
        assert_eq!((fb.pixel(64, 0), fb.pixel(65, 0), fb.pixel(66, 0)), (2, 1, 2));
        assert_eq!((fb.pixel(64, 1), fb.pixel(65, 1)), (1, 2));

        // Rect 3: palette RLE in both tiles.
        // tile A has 128 px, B has 12: pad with runs of index 0.
        let mut t = Vec::new();
        t.push(130);
        t.extend(cpx(0x0000ff));
        t.extend(cpx(0xffffff));
        t.push(1); // single white
        t.extend([0x80, 126]); // 127 blue
        t.push(130);
        t.extend(cpx(0x0000ff));
        t.extend(cpx(0xffffff));
        t.extend([0x81, 10]); // 11 white
        t.push(0); // 1 blue
        decode(&mut d, r, &enc.rect(&t), &mut fb).unwrap();
        assert_eq!(fb.pixel(0, 0), 0xffffff);
        assert_eq!(fb.pixel(1, 0), 0x0000ff);
        assert_eq!(fb.pixel(64 + 4, 1), 0xffffff);
        assert_eq!(fb.pixel(64 + 5, 1), 0x0000ff);
    }

    #[test]
    fn hostile_inputs_fail_cleanly() {
        let r = Rect { x: 0, y: 0, width: 4, height: 4 };
        let mut fb = Framebuffer::new(4, 4);
        let cases: Vec<Vec<u8>> = vec![
            vec![17],                                                   // unused subencoding
            vec![129],                                                  // unused subencoding
            vec![0, 1, 2],                                              // truncated raw
            vec![128, 1, 2, 3, 255, 255, 255],                          // run past the tile
            vec![130, 1, 1, 1, 2, 2, 2, 0x05],                          // palette index out of range
            vec![1, 0, 0, 0, 9],                                        // trailing garbage
            vec![3, 0, 0, 0, 1, 1, 1, 2, 2, 2, 0xff, 0xff, 0xff, 0xff], // index 3 with palette of 3
        ];
        for c in cases {
            let mut d = ZrleDecoder::default();
            let z = Enc::new().rect(&c);
            assert!(decode(&mut d, r, &z, &mut fb).is_err(), "accepted {c:?}");
        }
        // Not zlib at all.
        assert!(decode(&mut ZrleDecoder::default(), r, &[1, 2, 3, 4], &mut fb).is_err());
    }

    #[test]
    fn zip_bomb_is_refused() {
        let r = Rect { x: 0, y: 0, width: 1, height: 1 };
        let mut fb = Framebuffer::new(1, 1);
        let z = Enc::new().rect(&vec![0u8; 10 * 1024 * 1024]);
        assert!(z.len() < 64 * 1024);
        assert!(decode(&mut ZrleDecoder::default(), r, &z, &mut fb).is_err());
    }

    #[test]
    fn cpixel_layouts() {
        assert!(matches!(CPixel::of(&PF), CPixel::Ls3));
        let ms = PixelFormat { red_shift: 24, green_shift: 16, blue_shift: 8, ..PF };
        assert!(matches!(CPixel::of(&ms), CPixel::Ms3));
        let p16 = PixelFormat { bits_per_pixel: 16, depth: 16, ..PF };
        assert!(matches!(CPixel::of(&p16), CPixel::Full(2)));
    }

    /// When the output buffer fills mid-window, zlib keeps the rest of the
    /// window, and the next call only copies that out without reading input.
    /// That is progress, not a stall (wayvnc's 1.6 MB raw updates hit this).
    #[test]
    fn output_only_calls_count_as_progress() {
        // Mostly noisy pixels with short runs, like a real desktop
        // screenshot sent as raw ZRLE tiles: barely compressible.
        let mut raw = vec![0u8; 3 << 20];
        let mut x = 1u32;
        for (i, b) in raw.iter_mut().enumerate() {
            if i % 97 < 80 {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *b = (x >> 24) as u8;
            }
        }
        let mut enc = Enc::new();
        let mut z = Decompress::new(true);
        let mut out = Vec::new();
        // A small earlier rect on the same stream leaves zlib's 32 KiB window
        // offset, so output-buffer boundaries no longer line up with it.
        let small = vec![7u8; 1000];
        inflate(&mut z, &enc.rect(&small), &mut out, small.len()).unwrap();
        assert!(out == small);
        let compressed = enc.rect(&raw);
        inflate(&mut z, &compressed, &mut out, raw.len()).unwrap();
        assert!(out == raw, "inflated bytes differ");
    }
}
