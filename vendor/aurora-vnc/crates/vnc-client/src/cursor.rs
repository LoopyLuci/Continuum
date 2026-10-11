//! Cursor pseudo-encoding (RFC 6143 §7.8.1): the server sends the pointer's
//! shape so the client can draw it locally, with no round trip per move.
//!
//! Payload: `width * height` pixels in the session pixel format, then a
//! bitmask of `ceil(width / 8) * height` bytes, rows padded to whole bytes,
//! most significant bit first; a set bit means the pixel is opaque. The
//! rectangle's x and y are the hotspot. A 0x0 cursor means "hide it".

use rfb_proto::PixelFormat;

use crate::framebuffer::{read_pixel, to_rgb};

/// Largest cursor we accept on either side. Real cursors are at most a few
/// hundred pixels; this bounds what a hostile server can make us allocate.
pub const MAX_CURSOR_SIDE: u16 = 1024;

/// A pointer shape ready to hand to a window system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    pub width: u16,
    pub height: u16,
    /// Hotspot: the pixel that is "the point", relative to the top left.
    pub hot_x: u16,
    pub hot_y: u16,
    /// `width * height` pixels as 0xAARRGGBB, with alpha 0 or 255.
    pub argb: Vec<u32>,
}

impl CursorShape {
    /// True for the 0x0 shape a server sends to hide the pointer.
    pub fn is_hidden(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// RGBA bytes in row order, as most window systems want them.
    pub fn rgba(&self) -> Vec<u8> {
        self.argb.iter().flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]).collect()
    }
}

fn mask_stride(width: u16) -> usize {
    (width as usize).div_ceil(8)
}

/// Bytes of payload for a `width` x `height` cursor.
pub fn payload_len(pf: &PixelFormat, width: u16, height: u16) -> Result<usize, String> {
    if width > MAX_CURSOR_SIDE || height > MAX_CURSOR_SIDE {
        return Err(format!("cursor of {width}x{height} exceeds the {MAX_CURSOR_SIDE}x{MAX_CURSOR_SIDE} limit"));
    }
    let bpp = pf.bytes_per_pixel();
    Ok(width as usize * height as usize * bpp + mask_stride(width) * height as usize)
}

/// Decodes a payload of exactly [`payload_len`] bytes.
pub fn decode(
    data: &[u8],
    pf: &PixelFormat,
    colour_map: &[u32],
    hot_x: u16,
    hot_y: u16,
    width: u16,
    height: u16,
) -> Result<CursorShape, String> {
    let need = payload_len(pf, width, height)?;
    if data.len() != need {
        return Err(format!("cursor payload is {} bytes, expected {need}", data.len()));
    }
    // A hotspot outside the image is meaningless; clamp it rather than fail,
    // since some servers send (w, h) for "bottom right".
    let (hot_x, hot_y) = (hot_x.min(width.saturating_sub(1)), hot_y.min(height.saturating_sub(1)));
    let bpp = pf.bytes_per_pixel();
    let (w, h) = (width as usize, height as usize);
    let (pixels, mask) = data.split_at(w * h * bpp);
    let stride = mask_stride(width);
    let mut argb = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let rgb = to_rgb(read_pixel(&pixels[i * bpp..(i + 1) * bpp], pf), pf, colour_map);
            let opaque = mask[y * stride + x / 8] & (0x80 >> (x % 8)) != 0;
            argb.push(if opaque { 0xff00_0000 | rgb } else { 0 });
        }
    }
    Ok(CursorShape { width, height, hot_x, hot_y, argb })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGBX: PixelFormat = PixelFormat::RGBX8888_LE;

    /// Builds a payload independently of `decode`: `px(x, y)` gives 0xRRGGBB
    /// or `None` for transparent.
    fn encode(pf: &PixelFormat, w: u16, h: u16, px: impl Fn(u16, u16) -> Option<u32>) -> Vec<u8> {
        let mut pixels = Vec::new();
        let mut mask = vec![0u8; mask_stride(w) * h as usize];
        for y in 0..h {
            for x in 0..w {
                let c = px(x, y);
                let v = c.unwrap_or(0x123456);
                let (r, g, b) = ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
                let wire = ((r * pf.red_max as u32 / 255) << pf.red_shift)
                    | ((g * pf.green_max as u32 / 255) << pf.green_shift)
                    | ((b * pf.blue_max as u32 / 255) << pf.blue_shift);
                let bytes = wire.to_le_bytes();
                match (pf.bytes_per_pixel(), pf.big_endian) {
                    (4, false) => pixels.extend_from_slice(&bytes),
                    (4, true) => pixels.extend(bytes.iter().rev()),
                    (2, false) => pixels.extend_from_slice(&bytes[..2]),
                    (2, true) => pixels.extend([bytes[1], bytes[0]]),
                    _ => pixels.push(bytes[0]),
                }
                if c.is_some() {
                    mask[y as usize * mask_stride(w) + x as usize / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        pixels.extend(mask);
        pixels
    }

    #[test]
    fn arrow_with_odd_width_round_trips() {
        // 11 wide so mask rows need padding; a triangle "arrow".
        let (w, h) = (11, 17);
        let shape = |x: u16, y: u16| (x <= y).then_some(if x == 0 || x == y { 0x000000 } else { 0xffffff });
        for pf in [RGBX, PixelFormat { big_endian: true, ..RGBX }] {
            let data = encode(&pf, w, h, shape);
            assert_eq!(data.len(), payload_len(&pf, w, h).unwrap());
            let c = decode(&data, &pf, &[], 1, 2, w, h).unwrap();
            assert_eq!((c.hot_x, c.hot_y, c.width, c.height), (1, 2, w, h));
            for y in 0..h {
                for x in 0..w {
                    let want = shape(x, y).map_or(0, |v| 0xff00_0000 | v);
                    assert_eq!(c.argb[(y * w + x) as usize], want, "({x},{y})");
                }
            }
            assert_eq!(&c.rgba()[..4], &[0, 0, 0, 255]);
        }
    }

    #[test]
    fn rgb565_cursor() {
        let pf = PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..RGBX
        };
        let data = encode(&pf, 3, 1, |x, _| [Some(0xff0000), None, Some(0x0000ff)][x as usize]);
        let c = decode(&data, &pf, &[], 0, 0, 3, 1).unwrap();
        assert_eq!(c.argb, vec![0xffff0000, 0, 0xff0000ff]);
    }

    #[test]
    fn hidden_cursor_and_hotspot_clamp() {
        let c = decode(&[], &RGBX, &[], 0, 0, 0, 0).unwrap();
        assert!(c.is_hidden());
        let data = encode(&RGBX, 2, 2, |_, _| Some(0));
        let c = decode(&data, &RGBX, &[], 9, 9, 2, 2).unwrap();
        assert_eq!((c.hot_x, c.hot_y), (1, 1));
    }

    #[test]
    fn hostile_sizes_are_rejected() {
        assert!(payload_len(&RGBX, MAX_CURSOR_SIDE + 1, 1).is_err());
        assert!(payload_len(&RGBX, 1, u16::MAX).is_err());
        assert!(decode(&[0; 6], &RGBX, &[], 0, 0, 1, 1).is_err());
    }
}
