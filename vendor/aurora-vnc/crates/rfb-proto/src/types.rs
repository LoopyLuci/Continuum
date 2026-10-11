//! Core RFB value types shared by the handshake and the message codecs.

use crate::codec::{len_u32, DecodeError, Limits, PutBe, Reader};

/// A negotiated protocol version. Aurora advertises 3.8, the current RFC 6143
/// version. A peer that offers 3.8 or anything newer (3.889, 4.x) is spoken
/// to with the 3.8 message rules: a security-type list and a reason string
/// on failure. Only 3.3 and 3.7 keep their older rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    V3_3,
    V3_7,
    V3_8,
}

impl Version {
    pub const WIRE_LEN: usize = 12;

    pub fn wire(self) -> &'static [u8; 12] {
        match self {
            Version::V3_3 => b"RFB 003.003\n",
            Version::V3_7 => b"RFB 003.007\n",
            Version::V3_8 => b"RFB 003.008\n",
        }
    }

    pub fn encode(self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.wire());
    }

    /// 3.8 and every later version use the security-type list and failure reason.
    pub fn uses_38_security(self) -> bool {
        self >= Version::V3_8
    }

    /// Parse a 12-byte ProtocolVersion. 3.8, later 3.x, and any major above 3
    /// map to [`Version::V3_8`]: we never advertise above 3.8, and we do not
    /// drop a newer peer back to the 3.3 single-type handshake.
    pub fn decode(buf: &[u8]) -> Result<(Version, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let b = r.bytes(Self::WIRE_LEN)?;
        let digits = |s: &[u8]| -> Option<u32> {
            if s.iter().all(u8::is_ascii_digit) {
                std::str::from_utf8(s).ok()?.parse().ok()
            } else {
                None
            }
        };
        if &b[0..4] != b"RFB " || b[7] != b'.' || b[11] != b'\n' {
            return Err(DecodeError::Invalid("not an RFB ProtocolVersion".into()));
        }
        let (major, minor) = match (digits(&b[4..7]), digits(&b[8..11])) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err(DecodeError::Invalid("non-numeric ProtocolVersion".into())),
        };
        let v = match (major, minor) {
            (3, m) if m >= 8 => Version::V3_8,
            (3, 7) => Version::V3_7,
            (3, _) => Version::V3_3,
            (m, _) if m > 3 => Version::V3_8,
            _ => return Err(DecodeError::Invalid(format!("unsupported RFB version {major}.{minor}"))),
        };
        Ok((v, r.consumed()))
    }
}

/// RFB security types (IANA "RFB Security Types" registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityType {
    Invalid,
    None,
    VncAuth,
    Tight,
    VeNCrypt,
    Other(u8),
}

impl SecurityType {
    pub fn code(self) -> u8 {
        match self {
            SecurityType::Invalid => 0,
            SecurityType::None => 1,
            SecurityType::VncAuth => 2,
            SecurityType::Tight => 16,
            SecurityType::VeNCrypt => 19,
            SecurityType::Other(c) => c,
        }
    }

    pub fn from_code(c: u8) -> Self {
        match c {
            0 => SecurityType::Invalid,
            1 => SecurityType::None,
            2 => SecurityType::VncAuth,
            16 => SecurityType::Tight,
            19 => SecurityType::VeNCrypt,
            c => SecurityType::Other(c),
        }
    }
}

/// RFB encoding numbers, including pseudo-encodings (IANA "RFB Encoding Types").
/// Unknown values are preserved so they can be ignored rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Raw,
    CopyRect,
    Rre,
    Hextile,
    Tight,
    Trle,
    Zrle,
    /// Pseudo-encoding: client can handle cursor shape updates.
    Cursor,
    /// Pseudo-encoding: client can handle framebuffer size changes.
    DesktopSize,
    /// Pseudo-encoding: ExtendedDesktopSize (multi-screen, client-requested resize).
    ExtendedDesktopSize,
    /// Pseudo-encoding: cursor shape as 32-bit RGBA with premultiplied alpha.
    CursorWithAlpha,
    /// Pseudo-encoding: the extended ClientCutText/ServerCutText format
    /// (UTF-8, formats, caps/request/peek/notify/provide).
    ExtendedClipboard,
    /// Aurora Video (0x41555200): tiled AV1/HEVC/H.264 rectangles plus the
    /// Aurora message type 65; see `crate::video`.
    AuroraVideo,
    Other(i32),
}

/// 0xC0A1E5CE as a signed 32-bit encoding number.
const EXTENDED_CLIPBOARD_CODE: i32 = 0xC0A1_E5CEu32 as i32;

impl Encoding {
    pub fn code(self) -> i32 {
        match self {
            Encoding::Raw => 0,
            Encoding::CopyRect => 1,
            Encoding::Rre => 2,
            Encoding::Hextile => 5,
            Encoding::Tight => 7,
            Encoding::Trle => 15,
            Encoding::Zrle => 16,
            Encoding::Cursor => -239,
            Encoding::DesktopSize => -223,
            Encoding::ExtendedDesktopSize => -308,
            Encoding::CursorWithAlpha => -314,
            Encoding::ExtendedClipboard => EXTENDED_CLIPBOARD_CODE,
            Encoding::AuroraVideo => crate::video::VIDEO_ENCODING,
            Encoding::Other(c) => c,
        }
    }

    pub fn from_code(c: i32) -> Self {
        match c {
            0 => Encoding::Raw,
            1 => Encoding::CopyRect,
            2 => Encoding::Rre,
            5 => Encoding::Hextile,
            7 => Encoding::Tight,
            15 => Encoding::Trle,
            16 => Encoding::Zrle,
            -239 => Encoding::Cursor,
            -223 => Encoding::DesktopSize,
            -308 => Encoding::ExtendedDesktopSize,
            -314 => Encoding::CursorWithAlpha,
            EXTENDED_CLIPBOARD_CODE => Encoding::ExtendedClipboard,
            crate::video::VIDEO_ENCODING => Encoding::AuroraVideo,
            c => Encoding::Other(c),
        }
    }
}

/// PIXEL_FORMAT, RFC 6143 section 7.4. Flags decode as "nonzero is true" and
/// encode as 1, so a peer that sends 0xff (x11vnc does) round-trips to 0x01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    pub const WIRE_LEN: usize = 16;

    /// 32 bpp, depth 24, little-endian, 8 bits per channel: the de facto default.
    pub const RGBX8888_LE: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    };

    pub fn bytes_per_pixel(&self) -> usize {
        (self.bits_per_pixel as usize).div_ceil(8)
    }

    pub fn validate(&self) -> Result<(), DecodeError> {
        if !matches!(self.bits_per_pixel, 8 | 16 | 32) {
            return Err(DecodeError::Invalid(format!("bits-per-pixel {} not 8/16/32", self.bits_per_pixel)));
        }
        if self.depth == 0 || self.depth > self.bits_per_pixel {
            return Err(DecodeError::Invalid(format!("depth {} invalid for {} bpp", self.depth, self.bits_per_pixel)));
        }
        Ok(())
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u8(self.bits_per_pixel);
        out.put_u8(self.depth);
        out.put_u8(self.big_endian as u8);
        out.put_u8(self.true_colour as u8);
        out.put_u16(self.red_max);
        out.put_u16(self.green_max);
        out.put_u16(self.blue_max);
        out.put_u8(self.red_shift);
        out.put_u8(self.green_shift);
        out.put_u8(self.blue_shift);
        out.extend_from_slice(&[0, 0, 0]);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let pf = PixelFormat {
            bits_per_pixel: r.u8()?,
            depth: r.u8()?,
            big_endian: r.u8()? != 0,
            true_colour: r.u8()? != 0,
            red_max: r.u16()?,
            green_max: r.u16()?,
            blue_max: r.u16()?,
            red_shift: r.u8()?,
            green_shift: r.u8()?,
            blue_shift: r.u8()?,
        };
        r.skip(3)?;
        pf.validate()?;
        Ok(pf)
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let pf = Self::read(&mut r)?;
        Ok((pf, r.consumed()))
    }
}

/// ClientInit, RFC 6143 section 7.3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientInit {
    pub shared: bool,
}

impl ClientInit {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u8(self.shared as u8);
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let shared = r.u8()? != 0;
        Ok((ClientInit { shared }, r.consumed()))
    }
}

/// ServerInit, RFC 6143 section 7.3.2. The name is raw bytes: RFC 6143 says
/// Latin-1 in practice but many servers send UTF-8, so we do not guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInit {
    pub width: u16,
    pub height: u16,
    pub pixel_format: PixelFormat,
    pub name: Vec<u8>,
}

impl ServerInit {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u16(self.width);
        out.put_u16(self.height);
        self.pixel_format.encode(out);
        out.put_u32(len_u32(self.name.len(), "desktop name"));
        out.extend_from_slice(&self.name);
    }

    pub fn decode(buf: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let width = r.u16()?;
        let height = r.u16()?;
        let pixel_format = PixelFormat::read(&mut r)?;
        let name = r.len_prefixed(limits.max_name_len, "desktop name")?.to_vec();
        Ok((ServerInit { width, height, pixel_format, name }, r.consumed()))
    }
}
