//! Client-side framebuffer. Pixels are stored as `u32` 0x00RRGGBB regardless
//! of the wire format, so renderers and tests never deal with pixel formats.

use rfb_proto::PixelFormat;

/// An axis-aligned rectangle in framebuffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// True when the rectangle lies entirely inside a `w` x `h` surface.
    pub fn fits(&self, w: u16, h: u16) -> bool {
        self.x as u32 + self.width as u32 <= w as u32 && self.y as u32 + self.height as u32 <= h as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Framebuffer { width, height, pixels: vec![0; width as usize * height as usize] }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Row-major 0x00RRGGBB pixels, `width * height` long.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixel(&self, x: u16, y: u16) -> u32 {
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Resize, keeping the overlapping top-left region and clearing the rest.
    pub fn resize(&mut self, width: u16, height: u16) {
        let mut next = Framebuffer::new(width, height);
        let cw = self.width.min(width) as usize;
        for y in 0..self.height.min(height) as usize {
            let src = y * self.width as usize;
            let dst = y * width as usize;
            next.pixels[dst..dst + cw].copy_from_slice(&self.pixels[src..src + cw]);
        }
        *self = next;
    }

    /// Writes a Raw-encoded rectangle. Caller guarantees `r` fits and `data`
    /// is exactly `r.area() * bytes_per_pixel` long.
    pub fn put_raw(&mut self, r: Rect, data: &[u8], pf: &PixelFormat, colour_map: &[u32]) {
        let bpp = pf.bytes_per_pixel();
        let row_bytes = r.width as usize * bpp;
        let fbw = self.width as usize;
        for row in 0..r.height as usize {
            let src = &data[row * row_bytes..(row + 1) * row_bytes];
            let dst_start = (r.y as usize + row) * fbw + r.x as usize;
            let dst = &mut self.pixels[dst_start..dst_start + r.width as usize];
            // Fast path for the format we normally request.
            if *pf == PixelFormat::RGBX8888_LE {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(4)) {
                    *d = u32::from_le_bytes([s[0], s[1], s[2], 0]);
                }
            } else {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(bpp)) {
                    *d = to_rgb(read_pixel(s, pf), pf, colour_map);
                }
            }
        }
    }

    /// Fills `r` with one 0x00RRGGBB colour. Caller guarantees `r` fits.
    pub fn fill(&mut self, r: Rect, rgb: u32) {
        let fbw = self.width as usize;
        for row in 0..r.height as usize {
            let d = (r.y as usize + row) * fbw + r.x as usize;
            self.pixels[d..d + r.width as usize].fill(rgb);
        }
    }

    /// Writes already-converted 0x00RRGGBB pixels (row-major, `r.area()` long).
    pub fn put_rgb(&mut self, r: Rect, rgb: &[u32]) {
        let fbw = self.width as usize;
        let w = r.width as usize;
        for row in 0..r.height as usize {
            let d = (r.y as usize + row) * fbw + r.x as usize;
            self.pixels[d..d + w].copy_from_slice(&rgb[row * w..(row + 1) * w]);
        }
    }

    /// CopyRect: copy `w` x `h` from (`sx`, `sy`) to `dst`. Overlap-safe.
    /// Caller guarantees both source and destination fit.
    pub fn copy_rect(&mut self, dst: Rect, sx: u16, sy: u16) {
        let fbw = self.width as usize;
        let w = dst.width as usize;
        let rows: Box<dyn Iterator<Item = usize>> =
            if dst.y > sy { Box::new((0..dst.height as usize).rev()) } else { Box::new(0..dst.height as usize) };
        for row in rows {
            let s = (sy as usize + row) * fbw + sx as usize;
            let d = (dst.y as usize + row) * fbw + dst.x as usize;
            self.pixels.copy_within(s..s + w, d);
        }
    }

    /// Binary PPM (P6), dependency-free and readable by every image tool.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.reserve(self.pixels.len() * 3);
        for p in &self.pixels {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        out
    }
}

pub(crate) fn read_pixel(s: &[u8], pf: &PixelFormat) -> u32 {
    match (s.len(), pf.big_endian) {
        (1, _) => s[0] as u32,
        (2, false) => u16::from_le_bytes([s[0], s[1]]) as u32,
        (2, true) => u16::from_be_bytes([s[0], s[1]]) as u32,
        (_, false) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        (_, true) => u32::from_be_bytes([s[0], s[1], s[2], s[3]]),
    }
}

fn scale(v: u32, max: u16) -> u32 {
    if max == 0 {
        return 0;
    }
    let v = v.min(max as u32);
    (v * 255 + max as u32 / 2) / max as u32
}

/// Converts one wire pixel value to 0x00RRGGBB.
pub fn to_rgb(v: u32, pf: &PixelFormat, colour_map: &[u32]) -> u32 {
    if pf.true_colour {
        let r = scale((v >> pf.red_shift.min(31)) & pf.red_max as u32, pf.red_max);
        let g = scale((v >> pf.green_shift.min(31)) & pf.green_max as u32, pf.green_max);
        let b = scale((v >> pf.blue_shift.min(31)) & pf.blue_max as u32, pf.blue_max);
        (r << 16) | (g << 8) | b
    } else {
        colour_map.get(v as usize).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB565_LE: PixelFormat = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: false,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };

    #[test]
    fn rgb565_full_scale() {
        assert_eq!(to_rgb(0xf800, &RGB565_LE, &[]), 0xff0000);
        assert_eq!(to_rgb(0x07e0, &RGB565_LE, &[]), 0x00ff00);
        assert_eq!(to_rgb(0x001f, &RGB565_LE, &[]), 0x0000ff);
    }

    #[test]
    fn big_endian_32() {
        let pf = PixelFormat { big_endian: true, ..PixelFormat::RGBX8888_LE };
        let mut fb = Framebuffer::new(1, 1);
        fb.put_raw(Rect { x: 0, y: 0, width: 1, height: 1 }, &[0, 0x12, 0x34, 0x56], &pf, &[]);
        assert_eq!(fb.pixel(0, 0), 0x123456);
    }

    #[test]
    fn colour_map_lookup() {
        let pf = PixelFormat { bits_per_pixel: 8, depth: 8, true_colour: false, ..PixelFormat::RGBX8888_LE };
        let mut fb = Framebuffer::new(2, 1);
        fb.put_raw(Rect { x: 0, y: 0, width: 2, height: 1 }, &[1, 9], &pf, &[0, 0xabcdef]);
        assert_eq!(fb.pixels(), &[0xabcdef, 0]);
    }

    #[test]
    fn copy_rect_overlapping_down_and_right() {
        let mut fb = Framebuffer::new(4, 4);
        for (i, p) in fb.pixels.iter_mut().enumerate() {
            *p = i as u32;
        }
        let before = fb.clone();
        fb.copy_rect(Rect { x: 1, y: 1, width: 3, height: 3 }, 0, 0);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(fb.pixel(x + 1, y + 1), before.pixel(x, y));
            }
        }
    }

    #[test]
    fn copy_rect_overlapping_up() {
        let mut fb = Framebuffer::new(2, 4);
        for (i, p) in fb.pixels.iter_mut().enumerate() {
            *p = i as u32;
        }
        let before = fb.clone();
        fb.copy_rect(Rect { x: 0, y: 0, width: 2, height: 3 }, 0, 1);
        for y in 0..3 {
            assert_eq!(fb.pixel(0, y), before.pixel(0, y + 1));
        }
    }

    #[test]
    fn resize_keeps_overlap() {
        let mut fb = Framebuffer::new(2, 2);
        fb.pixels.copy_from_slice(&[1, 2, 3, 4]);
        fb.resize(3, 1);
        assert_eq!(fb.pixels(), &[1, 2, 0]);
    }
}
