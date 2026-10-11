//! RRE and Hextile encoders for viewers that did not advertise Tight or ZRLE.
//!
//! Pixels are 0x00RRGGBB. The payload is what follows the rectangle header.
//! Hextile tiles that would need many colours are sent raw.

use crate::codec::PutBe;

pub fn encode_rre(width: u16, height: u16, pixels: &[u32], out: &mut Vec<u8>) {
    let bg = pixels.first().copied().unwrap_or(0);
    let mut subs = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let px = pixels[y as usize * width as usize + x as usize];
            if px != bg {
                subs.extend_from_slice(&px.to_be_bytes());
                subs.extend_from_slice(&x.to_be_bytes());
                subs.extend_from_slice(&y.to_be_bytes());
                subs.extend_from_slice(&1u16.to_be_bytes());
                subs.extend_from_slice(&1u16.to_be_bytes());
            }
        }
    }
    out.put_u32(subs.len() as u32 / 12);
    out.extend_from_slice(&bg.to_be_bytes());
    out.extend_from_slice(&subs);
}

pub fn encode_hextile(width: u16, height: u16, pixels: &[u32], out: &mut Vec<u8>) {
    let mut y = 0;
    while y < height {
        let mut x = 0;
        let th = (height - y).min(16);
        while x < width {
            let tw = (width - x).min(16);
            let mut tile = Vec::new();
            for row in 0..th {
                for col in 0..tw {
                    tile.push(pixels[(y + row) as usize * width as usize + (x + col) as usize]);
                }
            }
            let distinct = {
                let mut v = tile.clone();
                v.sort_unstable();
                v.dedup();
                v.len()
            };
            if distinct > 4 {
                out.push(1); // raw
                for px in &tile {
                    out.extend_from_slice(&px.to_be_bytes());
                }
            } else {
                let bg = tile[0];
                out.push(2); // background specified
                out.extend_from_slice(&bg.to_be_bytes());
                let coloured: Vec<_> = tile.iter().enumerate().filter(|(_, p)| **p != bg).collect();
                if !coloured.is_empty() {
                    // rewrite mask: background + coloured subrects
                    let n = coloured.len().min(255) as u8;
                    *out.last_mut().unwrap() = 2 | 8 | 16;
                    out.push(n);
                    for (i, px) in coloured.into_iter().take(n as usize) {
                        let col = (i as u16 % tw) as u8;
                        let row = (i as u16 / tw) as u8;
                        out.extend_from_slice(&px.to_be_bytes());
                        out.push((col << 4) | row);
                        out.push(0); // 1x1
                    }
                }
            }
            x += tw;
        }
        y += th;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rre_background_and_one_pixel() {
        let mut pixels = vec![0x00ff_0000; 4];
        pixels[3] = 0x0000_00ff;
        let mut out = Vec::new();
        encode_rre(2, 2, &pixels, &mut out);
        assert_eq!(&out[0..4], &1u32.to_be_bytes());
        assert_eq!(&out[4..8], &0x00ff_0000u32.to_be_bytes());
    }

    #[test]
    fn hextile_raw_when_many_colours() {
        let pixels: Vec<u32> = (0..16).collect();
        let mut out = Vec::new();
        encode_hextile(4, 4, &pixels, &mut out);
        assert_eq!(out[0], 1);
        assert!(out.len() > 16 * 4);
    }
}
