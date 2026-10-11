//! Tight decoding (rfbproto "Tight Encoding", as sent by TightVNC, TigerVNC,
//! libvncserver/x11vnc and Aurora's own host): fill, JPEG, and basic
//! compression with the copy, palette and gradient filters over four
//! connection-long zlib streams.
//!
//! This is written independently of `rfb_proto::encodings::tight` on
//! purpose, so host and client check each other instead of sharing a bug.
//! A Tight payload carries no overall length, so [`payload_len`] walks the
//! headers first and the session only decodes once every byte is there.

use flate2::Decompress;
use rfb_proto::PixelFormat;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::framebuffer::{read_pixel, to_rgb, Framebuffer, Rect};
use crate::zrle::{inflate, ZrleError};

const FILL: u8 = 0x8;
const JPEG: u8 = 0x9;
const EXPLICIT_FILTER: u8 = 0x4;
const FILTER_COPY: u8 = 0;
const FILTER_PALETTE: u8 = 1;
const FILTER_GRADIENT: u8 = 2;
/// Payloads shorter than this are sent without zlib.
const MIN_TO_COMPRESS: usize = 12;
/// Compact lengths are at most 22 bits.
const MAX_COMPACT: usize = (1 << 22) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TightError(pub String);

impl std::fmt::Display for TightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tight: {}", self.0)
    }
}

impl From<ZrleError> for TightError {
    fn from(e: ZrleError) -> Self {
        TightError(e.0.trim_start_matches("ZRLE: ").to_string())
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, TightError> {
    Err(TightError(m.into()))
}

/// TPIXEL size: 3 bytes (R, G, B) for 32 bpp, depth 24, 8 bits per colour;
/// otherwise a full PIXEL.
fn tpixel_len(pf: &PixelFormat) -> usize {
    if is_rgb24(pf) {
        3
    } else {
        pf.bytes_per_pixel()
    }
}

fn is_rgb24(pf: &PixelFormat) -> bool {
    pf.true_colour
        && pf.bits_per_pixel == 32
        && pf.depth == 24
        && (pf.red_max, pf.green_max, pf.blue_max) == (255, 255, 255)
}

/// Parsed layout of one Tight payload.
struct Layout {
    ctl: u8,
    filter: u8,
    /// Byte range of the palette TPIXELs.
    palette: (usize, usize),
    /// Byte range of the (possibly compressed) data.
    data: (usize, usize),
    /// Bytes the data inflates to (unused for fill and JPEG).
    raw_len: usize,
    total: usize,
}

struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cur<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn skip(&mut self, n: usize) -> Option<(usize, usize)> {
        let start = self.pos;
        let end = start.checked_add(n)?;
        if end > self.b.len() {
            return None;
        }
        self.pos = end;
        Some((start, end))
    }
    fn compact_len(&mut self) -> Option<usize> {
        let b0 = self.u8()?;
        let mut n = (b0 & 0x7f) as usize;
        if b0 & 0x80 != 0 {
            let b1 = self.u8()?;
            n |= ((b1 & 0x7f) as usize) << 7;
            if b1 & 0x80 != 0 {
                n |= (self.u8()? as usize) << 14;
            }
        }
        Some(n)
    }
}

/// Validates the headers of a Tight payload at the start of `buf`.
/// `Ok(None)`: more bytes are needed. Nothing is decoded or changed.
fn layout(buf: &[u8], pf: &PixelFormat, w: u16, h: u16) -> Result<Option<Layout>, TightError> {
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    let tp = tpixel_len(pf);
    let mut c = Cur { b: buf, pos: 0 };
    macro_rules! need {
        ($e:expr) => {
            match $e {
                Some(v) => v,
                None => return Ok(None),
            }
        };
    }
    let ctl = need!(c.u8());
    let kind = ctl >> 4;
    let mut lay = Layout { ctl, filter: FILTER_COPY, palette: (0, 0), data: (0, 0), raw_len: 0, total: 0 };
    if kind == FILL {
        lay.data = need!(c.skip(tp));
        lay.total = c.pos;
        return Ok(Some(lay));
    }
    if kind == JPEG {
        let len = need!(c.compact_len());
        if len > MAX_COMPACT {
            return err("JPEG length out of range");
        }
        lay.data = need!(c.skip(len));
        lay.total = c.pos;
        return Ok(Some(lay));
    }
    if kind & 0x8 != 0 {
        return err(format!("compression type {kind:#x} is not defined"));
    }
    if kind & EXPLICIT_FILTER != 0 {
        lay.filter = need!(c.u8());
    }
    lay.raw_len = match lay.filter {
        FILTER_COPY => n * tp,
        FILTER_PALETTE => {
            let colours = need!(c.u8()) as usize + 1;
            lay.palette = need!(c.skip(colours * tp));
            if colours <= 2 {
                h * w.div_ceil(8)
            } else {
                n
            }
        }
        FILTER_GRADIENT => {
            if !pf.true_colour {
                return err("gradient filter needs a true-colour pixel format");
            }
            n * tp
        }
        f => return err(format!("filter {f} is not defined")),
    };
    if lay.raw_len < MIN_TO_COMPRESS {
        lay.data = need!(c.skip(lay.raw_len));
    } else {
        let len = need!(c.compact_len());
        if len > MAX_COMPACT {
            return err("compressed length out of range");
        }
        lay.data = need!(c.skip(len));
    }
    lay.total = c.pos;
    Ok(Some(lay))
}

/// Total length of the Tight payload at the start of `buf`, or `None` if
/// it is not all there yet.
pub fn payload_len(buf: &[u8], pf: &PixelFormat, w: u16, h: u16) -> Result<Option<usize>, TightError> {
    Ok(layout(buf, pf, w, h)?.map(|l| l.total))
}

#[derive(Default)]
pub struct TightDecoder {
    streams: [Option<Decompress>; 4],
    raw: Vec<u8>,
    px: Vec<u32>,
    jpeg_rects: u64,
}

impl TightDecoder {
    /// Rectangles that arrived as (lossy) JPEG so far.
    pub fn jpeg_rects(&self) -> u64 {
        self.jpeg_rects
    }

    /// Decodes one complete Tight payload into `fb`. Caller guarantees `r`
    /// fits in `fb` and `payload` is exactly [`payload_len`] bytes.
    pub fn decode(
        &mut self,
        r: Rect,
        payload: &[u8],
        pf: &PixelFormat,
        cmap: &[u32],
        fb: &mut Framebuffer,
    ) -> Result<(), TightError> {
        let Some(lay) = layout(payload, pf, r.width, r.height)? else { return err("payload truncated") };
        // The server reset these streams before compressing this rectangle.
        for (i, s) in self.streams.iter_mut().enumerate() {
            if lay.ctl & (1 << i) != 0 {
                *s = None;
            }
        }
        let tp = tpixel_len(pf);
        let tpixel = |b: &[u8]| -> u32 {
            if tp == 3 {
                0x00ff_ffff & u32::from_be_bytes([0, b[0], b[1], b[2]])
            } else {
                to_rgb(read_pixel(b, pf), pf, cmap)
            }
        };
        let data = &payload[lay.data.0..lay.data.1];
        match lay.ctl >> 4 {
            FILL => {
                fb.fill(r, tpixel(data));
                return Ok(());
            }
            JPEG => {
                self.jpeg_rects += 1;
                return self.jpeg(r, data, fb);
            }
            _ => {}
        }
        if lay.raw_len < MIN_TO_COMPRESS {
            self.raw.clear();
            self.raw.extend_from_slice(data);
        } else {
            let stream = ((lay.ctl >> 4) & 3) as usize;
            let z = self.streams[stream].get_or_insert_with(|| Decompress::new(true));
            inflate(z, data, &mut self.raw, lay.raw_len)?;
        }
        if self.raw.len() != lay.raw_len {
            return err(format!("data inflated to {} bytes, expected {}", self.raw.len(), lay.raw_len));
        }
        let (w, h) = (r.width as usize, r.height as usize);
        self.px.clear();
        match lay.filter {
            FILTER_COPY => self.px.extend(self.raw.chunks_exact(tp).map(tpixel)),
            FILTER_PALETTE => {
                let pal: Vec<u32> = payload[lay.palette.0..lay.palette.1].chunks_exact(tp).map(tpixel).collect();
                if pal.len() <= 2 {
                    for row in self.raw.chunks_exact(w.div_ceil(8)) {
                        for x in 0..w {
                            let i = ((row[x / 8] >> (7 - x % 8)) & 1) as usize;
                            let Some(&p) = pal.get(i) else { return err("palette index out of range") };
                            self.px.push(p);
                        }
                    }
                } else {
                    for &i in &self.raw {
                        let Some(&p) = pal.get(i as usize) else { return err("palette index out of range") };
                        self.px.push(p);
                    }
                }
            }
            _ => self.gradient(w, h, tp, pf, cmap),
        }
        fb.put_rgb(r, &self.px);
        Ok(())
    }

    /// Gradient filter: each colour component is sent as its difference from
    /// left + above - above-left (clamped to the component's range).
    fn gradient(&mut self, w: usize, h: usize, tp: usize, pf: &PixelFormat, cmap: &[u32]) {
        let (shifts, maxes): ([u32; 3], [u32; 3]) = if tp == 3 {
            ([16, 8, 0], [255; 3])
        } else {
            (
                [pf.red_shift as u32, pf.green_shift as u32, pf.blue_shift as u32],
                [pf.red_max as u32, pf.green_max as u32, pf.blue_max as u32],
            )
        };
        let read = |b: &[u8]| -> u32 {
            if tp == 3 {
                u32::from_be_bytes([0, b[0], b[1], b[2]])
            } else {
                read_pixel(b, pf)
            }
        };
        // Values in "v" layout (R,G,B at `shifts`), reconstructed row by row.
        let mut prev = vec![0u32; w];
        let mut cur = vec![0u32; w];
        for y in 0..h {
            for x in 0..w {
                let diff = read(&self.raw[(y * w + x) * tp..(y * w + x + 1) * tp]);
                let mut v = 0u32;
                for c in 0..3 {
                    let get = |p: u32| ((p >> shifts[c]) & maxes[c]) as i64;
                    let left = if x > 0 { get(cur[x - 1]) } else { 0 };
                    let up = if y > 0 { get(prev[x]) } else { 0 };
                    let ul = if x > 0 && y > 0 { get(prev[x - 1]) } else { 0 };
                    let pred = (left + up - ul).clamp(0, maxes[c] as i64) as u32;
                    let comp = (pred + ((diff >> shifts[c]) & maxes[c])) & maxes[c];
                    v |= comp << shifts[c];
                }
                cur[x] = v;
            }
            for &v in &cur {
                self.px.push(if tp == 3 { v } else { to_rgb(v, pf, cmap) });
            }
            std::mem::swap(&mut prev, &mut cur);
        }
    }

    fn jpeg(&mut self, r: Rect, data: &[u8], fb: &mut Framebuffer) -> Result<(), TightError> {
        let opts = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::RGB)
            .set_max_width(r.width as usize)
            .set_max_height(r.height as usize);
        let mut d = JpegDecoder::new_with_options(data, opts);
        let rgb = d.decode().map_err(|e| TightError(format!("JPEG: {e:?}")))?;
        let dims = d.dimensions();
        if dims != Some((r.width as usize, r.height as usize)) {
            return err(format!("JPEG is {dims:?}, rectangle is {}x{}", r.width, r.height));
        }
        if rgb.len() != r.area() as usize * 3 {
            return err("JPEG produced the wrong amount of pixel data");
        }
        self.px.clear();
        self.px.extend(rgb.chunks_exact(3).map(|p| u32::from_be_bytes([0, p[0], p[1], p[2]])));
        fb.put_rgb(r, &self.px);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rfb_proto::encodings::TightEncoder;

    fn formats() -> Vec<(&'static str, PixelFormat)> {
        let base = PixelFormat::RGBX8888_LE;
        vec![
            ("rgbx8888 le", base),
            ("rgbx8888 be", PixelFormat { big_endian: true, ..base }),
            ("bgrx8888", PixelFormat { red_shift: 0, blue_shift: 16, ..base }),
            ("colours in top byte", PixelFormat { red_shift: 24, green_shift: 16, blue_shift: 8, ..base }),
            (
                "rgb565",
                PixelFormat {
                    bits_per_pixel: 16,
                    depth: 16,
                    red_max: 31,
                    green_max: 63,
                    blue_max: 31,
                    red_shift: 11,
                    green_shift: 5,
                    blue_shift: 0,
                    ..base
                },
            ),
            (
                "bgr233",
                PixelFormat {
                    bits_per_pixel: 8,
                    depth: 8,
                    red_max: 7,
                    green_max: 7,
                    blue_max: 3,
                    red_shift: 0,
                    green_shift: 3,
                    blue_shift: 6,
                    ..base
                },
            ),
        ]
    }

    /// Pixel values (in `pf`) for a picture with fills, 2-colour, few-colour
    /// and many-colour areas, so every Tight path gets used.
    fn picture(pf: &PixelFormat, w: u16, h: u16, kind: u32) -> Vec<u32> {
        let pack = |r: u32, g: u32, b: u32| {
            ((r * pf.red_max as u32 / 255) << pf.red_shift)
                | ((g * pf.green_max as u32 / 255) << pf.green_shift)
                | ((b * pf.blue_max as u32 / 255) << pf.blue_shift)
        };
        (0..h as u32)
            .flat_map(|y| (0..w as u32).map(move |x| (x, y)))
            .map(|(x, y)| match kind {
                0 => pack(10, 200, 30),
                1 => {
                    if (x / 3 + y / 2) % 2 == 0 {
                        pack(255, 255, 255)
                    } else {
                        pack(0, 0, 0)
                    }
                }
                2 => pack((x % 5) * 50, (y % 3) * 100, 77),
                _ => pack((x * 7 + y * 3) % 256, (x * y) % 256, (x ^ y) % 256),
            })
            .collect()
    }

    #[test]
    fn decodes_aurora_tight_in_every_format_with_shared_streams() {
        for (name, pf) in formats() {
            let mut enc = TightEncoder::new(6);
            let mut dec = TightDecoder::default();
            let (w, h) = (61u16, 37u16);
            let mut fb = Framebuffer::new(w, h);
            let r = Rect { x: 0, y: 0, width: w, height: h };
            // Several rects in a row: streams must stay in step across them.
            for round in 0..3 {
                for kind in 0..4 {
                    let px = picture(&pf, w, h, kind + round);
                    let mut payload = Vec::new();
                    enc.encode(&pf, w, h, &px, &mut payload);
                    for cut in 0..payload.len() {
                        assert_eq!(payload_len(&payload[..cut], &pf, w, h), Ok(None), "{name}: prefix {cut}");
                    }
                    let mut padded = payload.clone();
                    padded.extend([0xde, 0xad]);
                    assert_eq!(payload_len(&padded, &pf, w, h), Ok(Some(payload.len())), "{name}");
                    dec.decode(r, &payload, &pf, &[], &mut fb).unwrap_or_else(|e| panic!("{name} kind {kind}: {e}"));
                    let want: Vec<u32> = px.iter().map(|&v| to_rgb(v, &pf, &[])).collect();
                    assert_eq!(fb.pixels(), &want[..], "{name} kind {kind} round {round}");
                }
            }
        }
    }

    #[test]
    fn gradient_filter_24_and_16_bit() {
        // 24-bit TPIXELs: hand-built gradient of a smooth ramp (sent uncompressed: 3 px = 9 bytes).
        let pf = PixelFormat::RGBX8888_LE;
        let mut fb = Framebuffer::new(3, 1);
        let r = Rect { x: 0, y: 0, width: 3, height: 1 };
        // Pixels (10,20,30) (12,22,32) (14,24,34): diffs from left neighbour.
        let p = [0x40, FILTER_GRADIENT, 10, 20, 30, 2, 2, 2, 2, 2, 2];
        TightDecoder::default().decode(r, &p, &pf, &[], &mut fb).unwrap();
        assert_eq!(fb.pixels(), &[0x0a141e, 0x0c1620, 0x0e1822]);

        // 2x2 at RGB565: prediction is left + up - upleft, per component.
        let pf = PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGBX8888_LE
        };
        let v = |r: u16, g: u16, b: u16| ((r << 11) | (g << 5) | b).to_le_bytes();
        let mut p = vec![0x40, FILTER_GRADIENT];
        // Targets: (1,2,3) (2,3,4) / (3,4,5) (31,63,31); diffs computed by hand.
        p.extend(v(1, 2, 3)); // pred 0
        p.extend(v(1, 1, 1)); // pred left (1,2,3)
        p.extend(v(2, 2, 2)); // pred up (1,2,3)
        p.extend(v(27, 58, 25)); // pred (4,5,6) -> diff to (31,63,31)
        let mut fb = Framebuffer::new(2, 2);
        let r = Rect { x: 0, y: 0, width: 2, height: 2 };
        // 8 bytes < 12: sent uncompressed.
        TightDecoder::default().decode(r, &p, &pf, &[], &mut fb).unwrap();
        let rgb = |r: u32, g: u32, b: u32| to_rgb((r << 11) | (g << 5) | b, &pf, &[]);
        assert_eq!(fb.pixels(), &[rgb(1, 2, 3), rgb(2, 3, 4), rgb(3, 4, 5), rgb(31, 63, 31)]);
    }

    #[test]
    fn jpeg_rect_decodes() {
        let jpg = include_bytes!("testdata/red-blue-16x8.jpg");
        let mut p = vec![JPEG << 4];
        let n = jpg.len();
        assert!(n < 1 << 14);
        p.extend([(n as u8 & 0x7f) | 0x80, (n >> 7) as u8]);
        p.extend_from_slice(jpg);
        let pf = PixelFormat::RGBX8888_LE;
        assert_eq!(payload_len(&p, &pf, 16, 8), Ok(Some(p.len())));
        let mut fb = Framebuffer::new(20, 10);
        TightDecoder::default().decode(Rect { x: 2, y: 1, width: 16, height: 8 }, &p, &pf, &[], &mut fb).unwrap();
        let close = |a: u32, b: u32| (0..3).all(|s| ((a >> (s * 8)) & 0xff).abs_diff((b >> (s * 8)) & 0xff) <= 24);
        assert!(close(fb.pixel(4, 4), 0xff0000), "{:06x}", fb.pixel(4, 4));
        assert!(close(fb.pixel(15, 4), 0x0000ff), "{:06x}", fb.pixel(15, 4));
        assert_eq!(fb.pixel(0, 0), 0);
        // A JPEG of the wrong size for its rectangle is refused.
        let mut fb = Framebuffer::new(32, 32);
        let r = Rect { x: 0, y: 0, width: 32, height: 32 };
        assert!(TightDecoder::default().decode(r, &p, &pf, &[], &mut fb).is_err());
    }

    #[test]
    fn hostile_payloads_fail_cleanly() {
        let pf = PixelFormat::RGBX8888_LE;
        let r = Rect { x: 0, y: 0, width: 8, height: 8 };
        let cases: Vec<Vec<u8>> = vec![
            vec![0xa0],                                                              // undefined compression type (PNG)
            vec![0x40, 7],                                                           // undefined filter
            vec![0x40, 1, 0, 1, 2, 3, 8, 1, 2, 3, 4],                                // 1-colour palette, bad zlib
            vec![0x90, 3, 1, 2, 3],                                                  // garbage JPEG
            vec![0x00, 0x10, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16], // not zlib
        ];
        for c in cases {
            let mut fb = Framebuffer::new(8, 8);
            let res = match payload_len(&c, &pf, 8, 8) {
                Ok(Some(n)) => TightDecoder::default().decode(r, &c[..n], &pf, &[], &mut fb).map(|_| ()),
                Ok(None) => Ok(()), // incomplete is fine: the session just waits
                Err(e) => Err(e),
            };
            assert!(res.is_err() || payload_len(&c, &pf, 8, 8) == Ok(None), "accepted {c:02x?}");
        }
        // Palette indices past the palette: 3 colours, index 3.
        let mut p = vec![0x40, FILTER_PALETTE, 2, 1, 1, 1, 2, 2, 2, 3, 3, 3];
        p.extend([3u8; 2]); // 2x1 rect: 2 index bytes, under 12 so uncompressed
        let mut fb = Framebuffer::new(2, 1);
        let r = Rect { x: 0, y: 0, width: 2, height: 1 };
        assert!(TightDecoder::default().decode(r, &p, &pf, &[], &mut fb).is_err());
    }
}
