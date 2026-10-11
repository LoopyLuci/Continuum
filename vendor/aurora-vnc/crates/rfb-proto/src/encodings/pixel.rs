//! How one pixel value is written on the wire: PIXEL (RFC 6143 7.4),
//! ZRLE's CPIXEL (7.7.5), and Tight's TPIXEL.
//!
//! Encoders and decoders here work on pixel *values* already in the
//! client's pixel format (the number a PIXEL encodes), so a round trip is
//! the identity and no colour conversion happens in this crate.

use crate::types::PixelFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compact {
    /// CPIXEL is the full PIXEL.
    Full,
    /// CPIXEL is the three least significant bytes.
    Low3,
    /// CPIXEL is the three most significant bytes.
    High3,
}

/// Wire forms of pixel values in one [`PixelFormat`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelCodec {
    bytes: usize,
    big_endian: bool,
    compact: Compact,
    /// Channel shifts when TPIXEL is 3 bytes of red, green, blue.
    tight_rgb: Option<[u8; 3]>,
}

fn channel_mask(max: u16, shift: u8) -> u64 {
    if shift >= 32 {
        u64::MAX // never fits anywhere: forces the full form
    } else {
        u64::from(max) << shift
    }
}

impl PixelCodec {
    /// Works for any format that passed [`PixelFormat::validate`]; other
    /// formats are treated as 32 bpp.
    pub fn new(pf: &PixelFormat) -> Self {
        let bytes = match pf.bits_per_pixel {
            8 => 1,
            16 => 2,
            _ => 4,
        };
        let masks = [
            channel_mask(pf.red_max, pf.red_shift),
            channel_mask(pf.green_max, pf.green_shift),
            channel_mask(pf.blue_max, pf.blue_shift),
        ];
        let all = masks.iter().fold(0u64, |a, m| a | m);
        let compact = if pf.true_colour && bytes == 4 && pf.depth <= 24 {
            if all <= 0x00ff_ffff {
                Compact::Low3
            } else if all <= 0xffff_ffff && all & 0xff == 0 {
                Compact::High3
            } else {
                Compact::Full
            }
        } else {
            Compact::Full
        };
        let tight_rgb = (pf.true_colour
            && bytes == 4
            && pf.depth == 24
            && [pf.red_max, pf.green_max, pf.blue_max] == [255; 3]
            && [pf.red_shift, pf.green_shift, pf.blue_shift].iter().all(|&s| s <= 24))
        .then_some([pf.red_shift, pf.green_shift, pf.blue_shift]);
        PixelCodec { bytes, big_endian: pf.big_endian, compact, tight_rgb }
    }

    pub fn pixel_len(&self) -> usize {
        self.bytes
    }

    pub fn cpixel_len(&self) -> usize {
        if self.compact == Compact::Full {
            self.bytes
        } else {
            3
        }
    }

    pub fn tpixel_len(&self) -> usize {
        if self.tight_rgb.is_some() {
            3
        } else {
            self.bytes
        }
    }

    fn put_n(&self, v: u32, n: usize, out: &mut Vec<u8>) {
        let le = v.to_le_bytes();
        if self.big_endian {
            out.extend(le[..n].iter().rev());
        } else {
            out.extend_from_slice(&le[..n]);
        }
    }

    fn get_n(&self, b: &[u8]) -> u32 {
        let mut le = [0u8; 4];
        if self.big_endian {
            for (d, s) in le.iter_mut().zip(b.iter().rev()) {
                *d = *s;
            }
        } else {
            le[..b.len()].copy_from_slice(b);
        }
        u32::from_le_bytes(le)
    }

    pub fn put_pixel(&self, v: u32, out: &mut Vec<u8>) {
        self.put_n(v, self.bytes, out);
    }

    /// `b` must hold exactly [`pixel_len`](Self::pixel_len) bytes.
    pub fn get_pixel(&self, b: &[u8]) -> u32 {
        self.get_n(b)
    }

    pub fn put_cpixel(&self, v: u32, out: &mut Vec<u8>) {
        match self.compact {
            Compact::Full => self.put_n(v, self.bytes, out),
            Compact::Low3 => self.put_n(v & 0x00ff_ffff, 3, out),
            Compact::High3 => self.put_n(v >> 8, 3, out),
        }
    }

    /// `b` must hold exactly [`cpixel_len`](Self::cpixel_len) bytes.
    pub fn get_cpixel(&self, b: &[u8]) -> u32 {
        match self.compact {
            Compact::Full | Compact::Low3 => self.get_n(b),
            Compact::High3 => self.get_n(b) << 8,
        }
    }

    pub fn put_tpixel(&self, v: u32, out: &mut Vec<u8>) {
        match self.tight_rgb {
            Some([r, g, b]) => out.extend_from_slice(&[(v >> r) as u8, (v >> g) as u8, (v >> b) as u8]),
            None => self.put_pixel(v, out),
        }
    }

    /// `b` must hold exactly [`tpixel_len`](Self::tpixel_len) bytes.
    pub fn get_tpixel(&self, b: &[u8]) -> u32 {
        match self.tight_rgb {
            Some([r, g, bl]) => (u32::from(b[0]) << r) | (u32::from(b[1]) << g) | (u32::from(b[2]) << bl),
            None => self.get_pixel(b),
        }
    }
}
