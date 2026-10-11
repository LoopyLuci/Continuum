//! Pseudo-encoding rectangles beyond RFC 6143, following the community RFB
//! specification (rfbproto):
//!
//! - [`ExtendedDesktopSize`] (-308): framebuffer size plus a screen layout,
//!   with a reason and status in the rectangle's x and y. Clients request
//!   changes with [`ClientMessage::SetDesktopSize`](crate::ClientMessage).
//! - [`Cursor`] (-239): cursor pixels in the client's pixel format plus a
//!   1-bit transparency mask; x and y are the hotspot.
//! - [`AlphaCursor`] (Cursor With Alpha, -314): an encoding number (Aurora
//!   sends and accepts Raw) and 32-bit RGBA pixels with premultiplied alpha.
//!
//! Each type writes its own [`RectHeader`] with `encode` and reads the
//! payload that follows a header with `decode`.

use crate::codec::{DecodeError, Limits, PutBe, Reader};
use crate::messages::RectHeader;
use crate::types::{Encoding, PixelFormat};

/// One screen (head) inside the framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Screen {
    /// Chosen by whichever side created the screen; unique in a layout.
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Unused so far: preserve unknown bits, zero for new screens.
    pub flags: u32,
}

impl Screen {
    pub const WIRE_LEN: usize = 16;

    /// One screen covering a whole `width x height` framebuffer.
    pub fn whole(id: u32, width: u16, height: u16) -> Self {
        Screen { id, x: 0, y: 0, width, height, flags: 0 }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u32(self.id);
        out.put_u16(self.x);
        out.put_u16(self.y);
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.put_u32(self.flags);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Screen { id: r.u32()?, x: r.u16()?, y: r.u16()?, width: r.u16()?, height: r.u16()?, flags: r.u32()? })
    }
}

/// Reads `n` screens after checking they are all present.
pub(crate) fn read_screens(r: &mut Reader<'_>, n: u8) -> Result<Vec<Screen>, DecodeError> {
    let bytes = r.bytes(usize::from(n) * Screen::WIRE_LEN)?;
    let mut r2 = Reader::new(bytes);
    (0..n).map(|_| Screen::read(&mut r2)).collect()
}

/// Why a screen layout is not acceptable for a `width x height`
/// framebuffer (the rules of the rfbproto Screen Model): at least one and at
/// most 255 screens, each non-empty and inside the framebuffer, ids unique.
pub fn validate_layout(width: u16, height: u16, screens: &[Screen]) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("framebuffer size {width}x{height} is empty"));
    }
    if screens.is_empty() || screens.len() > 255 {
        return Err(format!("{} screens (need 1 to 255)", screens.len()));
    }
    for (i, s) in screens.iter().enumerate() {
        if s.width == 0 || s.height == 0 {
            return Err(format!("screen {} is empty", s.id));
        }
        if u32::from(s.x) + u32::from(s.width) > u32::from(width)
            || u32::from(s.y) + u32::from(s.height) > u32::from(height)
        {
            return Err(format!("screen {} lies outside the {width}x{height} framebuffer", s.id));
        }
        if screens[..i].iter().any(|o| o.id == s.id) {
            return Err(format!("screen id {} appears twice", s.id));
        }
    }
    Ok(())
}

/// ExtendedDesktopSize reasons (the rectangle's x).
pub mod reason {
    /// Changed on the server, or the answer to a non-incremental request.
    pub const SERVER: u16 = 0;
    /// The receiving client asked for this change.
    pub const THIS_CLIENT: u16 = 1;
    /// Another client asked for this change and it was made.
    pub const OTHER_CLIENT: u16 = 2;
}

/// ExtendedDesktopSize status codes (the rectangle's y).
pub mod status {
    pub const OK: u16 = 0;
    pub const PROHIBITED: u16 = 1;
    pub const OUT_OF_RESOURCES: u16 = 2;
    pub const INVALID_LAYOUT: u16 = 3;
    /// The request was passed on; another rectangle follows if it succeeds.
    pub const FORWARDED: u16 = 4;
}

/// An ExtendedDesktopSize pseudo-rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedDesktopSize {
    /// See [`reason`]. Clients treat unknown values as [`reason::SERVER`].
    pub reason: u16,
    /// See [`status`]; only meaningful with [`reason::THIS_CLIENT`].
    pub status: u16,
    pub width: u16,
    pub height: u16,
    /// At most 255.
    pub screens: Vec<Screen>,
}

impl ExtendedDesktopSize {
    pub fn header(&self) -> RectHeader {
        RectHeader {
            x: self.reason,
            y: self.status,
            width: self.width,
            height: self.height,
            encoding: Encoding::ExtendedDesktopSize,
        }
    }

    /// Header and payload.
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.header().encode(out);
        let n = u8::try_from(self.screens.len()).expect("more than 255 screens");
        out.put_u8(n);
        out.extend_from_slice(&[0, 0, 0]);
        for s in &self.screens {
            s.encode(out);
        }
    }

    /// The payload following `h` (whose encoding must be ExtendedDesktopSize).
    pub fn decode(h: &RectHeader, buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let n = r.u8()?;
        r.skip(3)?;
        let screens = read_screens(&mut r, n)?;
        let eds = ExtendedDesktopSize { reason: h.x, status: h.y, width: h.width, height: h.height, screens };
        Ok((eds, r.consumed()))
    }
}

fn check_cursor_size(width: u16, height: u16, limits: &Limits) -> Result<(), DecodeError> {
    if width > limits.max_cursor_size || height > limits.max_cursor_size {
        return Err(DecodeError::Invalid(format!(
            "{width}x{height} cursor exceeds the {} pixel limit",
            limits.max_cursor_size
        )));
    }
    Ok(())
}

/// Bytes in a cursor bitmask: rows padded to whole bytes.
pub fn cursor_mask_len(width: u16, height: u16) -> usize {
    usize::from(width).div_ceil(8) * usize::from(height)
}

/// A Cursor (-239) pseudo-rectangle: pixel values in the client's format
/// and a bitmask (MSB first, rows padded to bytes; 1 = visible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub hot_x: u16,
    pub hot_y: u16,
    pub width: u16,
    pub height: u16,
    /// `width * height` pixel values in the client's pixel format.
    pub pixels: Vec<u32>,
    /// [`cursor_mask_len`] bytes.
    pub mask: Vec<u8>,
}

impl Cursor {
    pub fn header(&self) -> RectHeader {
        RectHeader { x: self.hot_x, y: self.hot_y, width: self.width, height: self.height, encoding: Encoding::Cursor }
    }

    /// Whether pixel (x, y) is visible according to the mask.
    pub fn visible(&self, x: u16, y: u16) -> bool {
        let row = usize::from(self.width).div_ceil(8);
        self.mask[usize::from(y) * row + usize::from(x) / 8] & (0x80 >> (x % 8)) != 0
    }

    pub fn encode(&self, pf: &PixelFormat, out: &mut Vec<u8>) {
        assert_eq!(self.pixels.len(), usize::from(self.width) * usize::from(self.height));
        assert_eq!(self.mask.len(), cursor_mask_len(self.width, self.height));
        self.header().encode(out);
        let codec = crate::encodings::PixelCodec::new(pf);
        for &p in &self.pixels {
            codec.put_pixel(p, out);
        }
        out.extend_from_slice(&self.mask);
    }

    pub fn decode(h: &RectHeader, pf: &PixelFormat, buf: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        check_cursor_size(h.width, h.height, limits)?;
        let n = usize::from(h.width) * usize::from(h.height);
        let bpp = pf.bytes_per_pixel();
        let mut r = Reader::new(buf);
        let px = r.bytes(n * bpp)?;
        let mask = r.bytes(cursor_mask_len(h.width, h.height))?.to_vec();
        let codec = crate::encodings::PixelCodec::new(pf);
        let pixels = px.chunks_exact(bpp).map(|c| codec.get_pixel(c)).collect();
        let c = Cursor { hot_x: h.x, hot_y: h.y, width: h.width, height: h.height, pixels, mask };
        Ok((c, r.consumed()))
    }
}

/// A Cursor With Alpha (-314) pseudo-rectangle. Pixels are RGBA, 8 bits
/// each, alpha premultiplied, carried in Raw encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphaCursor {
    pub hot_x: u16,
    pub hot_y: u16,
    pub width: u16,
    pub height: u16,
    /// `width * height * 4` bytes: R, G, B, A per pixel.
    pub rgba: Vec<u8>,
}

impl AlphaCursor {
    pub fn header(&self) -> RectHeader {
        RectHeader {
            x: self.hot_x,
            y: self.hot_y,
            width: self.width,
            height: self.height,
            encoding: Encoding::CursorWithAlpha,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        assert_eq!(self.rgba.len(), usize::from(self.width) * usize::from(self.height) * 4);
        self.header().encode(out);
        out.put_i32(Encoding::Raw.code());
        out.extend_from_slice(&self.rgba);
    }

    /// Only Raw is accepted inside; other encodings are refused as Invalid
    /// (Aurora clients never advertise this pseudo-encoding with others).
    pub fn decode(h: &RectHeader, buf: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        check_cursor_size(h.width, h.height, limits)?;
        let mut r = Reader::new(buf);
        let inner = Encoding::from_code(r.i32()?);
        if inner != Encoding::Raw {
            return Err(DecodeError::Invalid(format!("cursor with alpha in {inner:?} is not supported (only Raw)")));
        }
        let rgba = r.bytes(usize::from(h.width) * usize::from(h.height) * 4)?.to_vec();
        let c = AlphaCursor { hot_x: h.x, hot_y: h.y, width: h.width, height: h.height, rgba };
        Ok((c, r.consumed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_desktop_size_round_trips() {
        let eds = ExtendedDesktopSize {
            reason: reason::THIS_CLIENT,
            status: status::INVALID_LAYOUT,
            width: 1920,
            height: 1080,
            screens: vec![
                Screen::whole(7, 1280, 1080),
                Screen { id: 9, x: 1280, y: 0, width: 640, height: 480, flags: 1 },
            ],
        };
        let mut out = Vec::new();
        eds.encode(&mut out);
        assert_eq!(out.len(), 12 + 4 + 2 * 16);
        let (h, n) = RectHeader::decode(&out).unwrap();
        assert_eq!((h.x, h.y, h.encoding), (1, 3, Encoding::ExtendedDesktopSize));
        let (back, used) = ExtendedDesktopSize::decode(&h, &out[n..]).unwrap();
        assert_eq!((back, used), (eds, out.len() - n));
        assert_eq!(ExtendedDesktopSize::decode(&h, &out[n..n + 10]), Err(DecodeError::Incomplete { needed: 36 }));
    }

    #[test]
    fn layout_validation_follows_the_screen_model() {
        let ok = [Screen::whole(1, 100, 50)];
        assert!(validate_layout(100, 50, &ok).is_ok());
        assert!(validate_layout(100, 50, &[]).is_err());
        assert!(validate_layout(99, 50, &ok).is_err());
        assert!(validate_layout(100, 50, &[Screen::whole(1, 50, 50), Screen::whole(1, 10, 10)]).is_err());
        assert!(validate_layout(100, 50, &[Screen { width: 0, ..ok[0] }]).is_err());
        assert!(validate_layout(0, 0, &ok).is_err());
        let many: Vec<Screen> = (0..256).map(|i| Screen::whole(i, 1, 1)).collect();
        assert!(validate_layout(1, 1, &many).is_err());
    }

    #[test]
    fn cursors_round_trip_and_respect_the_size_limit() {
        let pf = PixelFormat::RGBX8888_LE;
        let c = Cursor {
            hot_x: 1,
            hot_y: 2,
            width: 9,
            height: 2,
            pixels: (0..18).map(|i| i * 0x010203).collect(),
            mask: vec![0xff, 0x80, 0x00, 0x00],
        };
        let mut out = Vec::new();
        c.encode(&pf, &mut out);
        assert_eq!(out.len(), 12 + 18 * 4 + 4);
        let (h, n) = RectHeader::decode(&out).unwrap();
        let limits = Limits::default();
        assert_eq!(Cursor::decode(&h, &pf, &out[n..], &limits).unwrap(), (c.clone(), out.len() - n));
        assert!(c.visible(8, 0) && !c.visible(0, 1));
        let tiny = Limits { max_cursor_size: 8, ..limits };
        assert!(matches!(Cursor::decode(&h, &pf, &out[n..], &tiny), Err(DecodeError::Invalid(_))));

        let a = AlphaCursor { hot_x: 0, hot_y: 3, width: 2, height: 2, rgba: (0..16).collect() };
        let mut out = Vec::new();
        a.encode(&mut out);
        let (h, n) = RectHeader::decode(&out).unwrap();
        assert_eq!(h.encoding, Encoding::CursorWithAlpha);
        assert_eq!(AlphaCursor::decode(&h, &out[n..], &limits).unwrap(), (a, out.len() - n));
        let mut zrle = out.clone();
        zrle[n..n + 4].copy_from_slice(&16i32.to_be_bytes());
        assert!(matches!(AlphaCursor::decode(&h, &zrle[n..], &limits), Err(DecodeError::Invalid(_))));
    }
}
