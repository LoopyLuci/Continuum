//! RRE and Hextile, the rectangle encodings older servers send.
//!
//! Neither has a length prefix, so [`rre_len`] and [`hextile_len`] walk the
//! bytes and return `Ok(None)` until the rectangle is complete.

use rfb_proto::PixelFormat;

use crate::framebuffer::{read_pixel, to_rgb, Framebuffer};
use crate::Rect;

#[derive(Debug)]
pub struct ClassicError(pub String);

impl std::fmt::Display for ClassicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(s: impl Into<String>) -> ClassicError {
    ClassicError(s.into())
}

fn pixel<'a>(buf: &'a [u8], at: &mut usize, bpp: usize) -> Result<&'a [u8], ClassicError> {
    let end = at.checked_add(bpp).ok_or_else(|| err("pixel overruns the rectangle"))?;
    let s = buf.get(*at..end).ok_or_else(|| err("truncated pixel"))?;
    *at = end;
    Ok(s)
}

fn rgb(buf: &[u8], at: &mut usize, pf: &PixelFormat, map: &[u32]) -> Result<u32, ClassicError> {
    let s = pixel(buf, at, pf.bytes_per_pixel())?;
    Ok(to_rgb(read_pixel(s, pf), pf, map))
}

/// Bytes of one RRE rectangle, or `None` if `buf` is short.
pub fn rre_len(buf: &[u8], pf: &PixelFormat) -> Result<Option<usize>, ClassicError> {
    let bpp = pf.bytes_per_pixel();
    if buf.len() < 4 + bpp {
        return Ok(None);
    }
    let n = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
    if n > 1_000_000 {
        return Err(err(format!("RRE subrect count {n} is implausible")));
    }
    let need = 4 + bpp + n * (bpp + 8);
    if buf.len() < need {
        Ok(None)
    } else {
        Ok(Some(need))
    }
}

pub fn decode_rre(
    r: Rect,
    buf: &[u8],
    pf: &PixelFormat,
    map: &[u32],
    fb: &mut Framebuffer,
) -> Result<(), ClassicError> {
    let mut at = 4;
    let bg = rgb(buf, &mut at, pf, map)?;
    fb.fill(r, bg);
    let n = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    for _ in 0..n {
        let colour = rgb(buf, &mut at, pf, map)?;
        let (x, y, w, h) = subrect_u16(buf, &mut at)?;
        let sub = Rect { x: r.x.saturating_add(x), y: r.y.saturating_add(y), width: w, height: h };
        if !sub.fits(fb.width(), fb.height())
            || x as u32 + w as u32 > r.width as u32
            || y as u32 + h as u32 > r.height as u32
        {
            return Err(err("RRE subrect falls outside its rectangle"));
        }
        fb.fill(sub, colour);
    }
    Ok(())
}

fn subrect_u16(buf: &[u8], at: &mut usize) -> Result<(u16, u16, u16, u16), ClassicError> {
    let s = buf.get(*at..*at + 8).ok_or_else(|| err("truncated RRE subrect"))?;
    *at += 8;
    Ok((
        u16::from_be_bytes([s[0], s[1]]),
        u16::from_be_bytes([s[2], s[3]]),
        u16::from_be_bytes([s[4], s[5]]),
        u16::from_be_bytes([s[6], s[7]]),
    ))
}

const RAW: u8 = 1;
const BG: u8 = 2;
const FG: u8 = 4;
const ANY: u8 = 8;
const COLOURED: u8 = 16;

pub fn hextile_len(buf: &[u8], pf: &PixelFormat, width: u16, height: u16) -> Result<Option<usize>, ClassicError> {
    match walk_hextile(buf, pf.bytes_per_pixel(), width, height) {
        Ok(n) => Ok(Some(n)),
        Err(Walk::Short) => Ok(None),
        Err(Walk::Bad(s)) => Err(err(s)),
    }
}

enum Walk {
    Short,
    Bad(String),
}

fn walk_hextile(buf: &[u8], bpp: usize, width: u16, height: u16) -> Result<usize, Walk> {
    let mut at = 0;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        let th = (height - y).min(16);
        while x < width {
            let tw = (width - x).min(16);
            if at >= buf.len() {
                return Err(Walk::Short);
            }
            let mask = buf[at];
            at += 1;
            // RFC 6143 7.7.4 defines exactly five subencoding bits: Raw,
            // BackgroundSpecified, ForegroundSpecified, AnySubrects and
            // SubrectsColoured. A mask with anything else set is malformed.
            //
            // This is the only way `Walk::Bad` can be produced. Without the
            // check an undefined bit is silently ignored, the walk desynchronises
            // from the real payload, and the caller is handed a length that
            // looks plausible and a frame that is quietly wrong -- which is the
            // failure mode this crate exists to catch. It also gives the
            // `Bad` arm above a producer, rather than leaving a variant that
            // nothing can construct.
            if mask & !(RAW | BG | FG | ANY | COLOURED) != 0 {
                return Err(Walk::Bad(format!(
                    "hextile subencoding mask {mask:#04x} sets undefined bits"
                )));
            }
            // Raw subsumes the other four: a raw tile carries pixels and the
            // remaining bits must not be set alongside it.
            if mask & RAW != 0 && mask & (BG | FG | ANY) != 0 {
                return Err(Walk::Bad(format!(
                    "hextile tile {mask:#04x} combines Raw with other subencodings"
                )));
            }
            if mask & RAW != 0 {
                let n = tw as usize * th as usize * bpp;
                if at + n > buf.len() {
                    return Err(Walk::Short);
                }
                at += n;
            } else {
                if mask & BG != 0 {
                    if at + bpp > buf.len() {
                        return Err(Walk::Short);
                    }
                    at += bpp;
                }
                if mask & FG != 0 {
                    if at + bpp > buf.len() {
                        return Err(Walk::Short);
                    }
                    at += bpp;
                }
                if mask & ANY != 0 {
                    if at >= buf.len() {
                        return Err(Walk::Short);
                    }
                    let n = buf[at] as usize;
                    at += 1;
                    let each = if mask & COLOURED != 0 { bpp + 2 } else { 2 };
                    let bytes = n * each;
                    if at + bytes > buf.len() {
                        return Err(Walk::Short);
                    }
                    at += bytes;
                }
            }
            x += tw;
        }
        y += th;
    }
    Ok(at)
}

pub fn decode_hextile(
    r: Rect,
    buf: &[u8],
    pf: &PixelFormat,
    map: &[u32],
    fb: &mut Framebuffer,
) -> Result<(), ClassicError> {
    let mut at = 0;
    let mut bg = 0;
    let mut fg = 0;
    let mut y = 0;
    while y < r.height {
        let mut x = 0;
        let th = (r.height - y).min(16);
        while x < r.width {
            let tw = (r.width - x).min(16);
            let mask = *buf.get(at).ok_or_else(|| err("truncated hextile tile"))?;
            at += 1;
            let tile = Rect { x: r.x + x, y: r.y + y, width: tw, height: th };
            if mask & RAW != 0 {
                let n = tw as usize * th as usize * pf.bytes_per_pixel();
                let data = buf.get(at..at + n).ok_or_else(|| err("truncated hextile raw tile"))?;
                at += n;
                fb.put_raw(tile, data, pf, map);
            } else {
                if mask & BG != 0 {
                    bg = rgb(buf, &mut at, pf, map)?;
                }
                fb.fill(tile, bg);
                if mask & FG != 0 {
                    fg = rgb(buf, &mut at, pf, map)?;
                }
                if mask & ANY != 0 {
                    let n = *buf.get(at).ok_or_else(|| err("truncated hextile subrect count"))? as usize;
                    at += 1;
                    for _ in 0..n {
                        let colour = if mask & COLOURED != 0 { rgb(buf, &mut at, pf, map)? } else { fg };
                        let xy = *buf.get(at).ok_or_else(|| err("truncated hextile subrect"))?;
                        let wh = *buf.get(at + 1).ok_or_else(|| err("truncated hextile subrect"))?;
                        at += 2;
                        let sub = Rect {
                            x: tile.x + u16::from(xy >> 4),
                            y: tile.y + u16::from(xy & 0x0f),
                            width: u16::from(wh >> 4) + 1,
                            height: u16::from(wh & 0x0f) + 1,
                        };
                        if !sub.fits(fb.width(), fb.height()) {
                            return Err(err("hextile subrect falls outside the framebuffer"));
                        }
                        fb.fill(sub, colour);
                    }
                }
            }
            x += tw;
        }
        y += th;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rfb_proto::PixelFormat;

    #[test]
    fn rre_fills_background_then_one_subrect() {
        let pf = PixelFormat::RGBX8888_LE;
        let mut body = vec![0, 0, 0, 1];
        body.extend_from_slice(&0x0000_00ff_u32.to_le_bytes());
        body.extend_from_slice(&0x0000_ff00_u32.to_le_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        assert_eq!(rre_len(&body, &pf).unwrap(), Some(body.len()));
        let mut fb = Framebuffer::new(4, 4);
        decode_rre(Rect { x: 0, y: 0, width: 4, height: 4 }, &body, &pf, &[], &mut fb).unwrap();
        assert_eq!(fb.pixels()[0], 0x0000_00ff);
        assert_eq!(fb.pixels()[5], 0x0000_ff00);
    }

    #[test]
    fn hextile_raw_tile_paints_one_pixel() {
        let pf = PixelFormat::RGBX8888_LE;
        let mut body = vec![RAW];
        body.extend_from_slice(&0x00ff_0000_u32.to_le_bytes());
        assert_eq!(hextile_len(&body, &pf, 1, 1).unwrap(), Some(body.len()));
        let mut fb = Framebuffer::new(1, 1);
        decode_hextile(Rect { x: 0, y: 0, width: 1, height: 1 }, &body, &pf, &[], &mut fb).unwrap();
        assert_eq!(fb.pixels()[0], 0x00ff_0000);
    }
}
