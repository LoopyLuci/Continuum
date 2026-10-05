//! RFB wire format: constants, negotiation, pixel formats, client messages.
//!
//! Version support is 3.3, 3.7 and 3.8. The differences that matter here are
//! all in the security handshake:
//!
//! * **3.8** — the server sends a length-prefixed list of security types and
//!   the client picks one. Authentication *failures* carry a reason string.
//! * **3.7** — same list mechanism, but a failure has no reason string.
//! * **3.3** — the server picks the security type itself and sends it as a
//!   single `u32`. This is why [`decide_version`] is not a version clamp:
//!   3.3 is materially older and gets a different code path.
//!
//! Everything after security is identical across the three versions, which is
//! why one server can speak all of them.
//!
//! Coordinates are `u16` on the wire throughout. A framebuffer wider or taller
//! than 65535 therefore cannot be described by RFB 3.x at all, and the server
//! refuses to advertise one rather than wrapping.

use crate::rfb::RfbError;

/// Default RFB port. Conventionally 5900 + display number, so a bare VNC server
/// lands on 5900.
pub const DEFAULT_PORT: u16 = 5900;

/// The exact banner a server sends. Trailing `\n` is required: clients read 12
/// bytes and a missing newline makes the reply arrive with a stray byte in
/// front of it.
pub const SERVER_VERSION_BANNER: &[u8; 12] = b"RFB 003.008\n";

/// Bytes in a `ProtocolVersion` message. Fixed by the wire format.
pub const MAX_VERSION_BANNER_BYTES: usize = 12;

/// Protocol versions this server speaks, as `major*100 + minor`.
/// Encoded as `major * 100 + minor`, so RFB 3.8 is 308. The convention matches
/// every existing VNC implementation and leaves 4.x unambiguous.
pub const SUPPORTED_VERSIONS: &[u16] = &[303, 307, 308];

/// Highest version this server offers.
pub const HIGHEST_SUPPORTED_VERSION: u16 = 308;

/// Security type: none.
pub const SECURITY_NONE: u8 = 1;

/// Security type: VNC Authentication.
pub const SECURITY_VNC_AUTH: u8 = 2;

/// Sentinel a 3.3 client sends when it rejects the server's chosen security
/// type, and the count a 3.8 client sends when it accepts the whole list.
pub const SECURITY_INVALID: u32 = 0xFFFF_FFFF;

/// Client-to-server message types.
pub mod client_messages {
    pub const SET_PIXEL_FORMAT: u8 = 0;
    pub const SET_ENCODINGS: u8 = 2;
    pub const FRAMEBUFFER_UPDATE_REQUEST: u8 = 3;
    pub const KEY_EVENT: u8 = 4;
    pub const POINTER_EVENT: u8 = 5;
    pub const CLIENT_CUT_TEXT: u8 = 6;
}

/// Server-to-client message types.
pub mod server_messages {
    pub const FRAMEBUFFER_UPDATE: u8 = 0;
    pub const SET_COLOUR_MAP_ENTRIES: u8 = 1;
    pub const BELL: u8 = 2;
}

/// Longest desktop name in a `ServerInit`.
pub const MAX_DESKTOP_NAME: usize = 256;

/// Longest `ClientCutText` payload the server will read.
pub const MAX_CUT_TEXT_BYTES: usize = 64 * 1024;

/// Most encodings accepted in one `SetEncodings`.
pub const MAX_CLIENT_ENCODINGS: usize = 1024;

/// Fallback desktop name when the configuration does not set one.
pub const DEFAULT_DESKTOP_NAME: &str = "Continuum QEMU";

/// The outcome of comparing a client's version against what we speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionDecision {
    /// Speak the client's version.
    Accept(u16),
    /// The client asked for something newer than 3.8. Re-send the banner; a
    /// conforming client retries with 3.8 and a non-conforming one is dropped
    /// after [`crate::rfb::MAX_VERSION_RETRIES`] attempts.
    Retry,
}

/// Parse the client's 12-byte version line into `major*100 + minor`.
///
/// A client that says something completely unparseable is a protocol error, not
/// a version negotiation failure: there is no sensible default to fall back
/// to, and guessing means speaking a dialect the client does not expect.
pub fn parse_client_version(buf: &[u8]) -> Result<u16, RfbError> {
    if buf.len() != 12 {
        return Err(RfbError::Protocol(format!(
            "version line was {} bytes, expected 12",
            buf.len()
        )));
    }
    if &buf[..4] != b"RFB " {
        return Err(RfbError::Protocol("version line did not start with \"RFB \"".into()));
    }
    // "RFB xxx.yyy\n": the major version is bytes 4..7, the minor is 8..11, and
    // byte 7 is the separator.
    let major_part = &buf[4..7];
    let minor_part = &buf[8..11];
    if buf[7] != b'.' {
        return Err(RfbError::Protocol(format!(
            "version number {:?} was not xxx.yyy",
            String::from_utf8_lossy(&buf[4..11])
        )));
    }
    for part in [major_part, minor_part] {
        if part.iter().any(|b| !b.is_ascii_digit()) {
            return Err(RfbError::Protocol(format!(
                "version number {:?} was not digits",
                String::from_utf8_lossy(&buf[4..11])
            )));
        }
    }
    let parse = |part: &[u8]| -> Result<u16, RfbError> {
        std::str::from_utf8(part)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| RfbError::Protocol("could not parse the version number".into()))
    };
    let major: u16 = parse(major_part)?;
    let minor: u16 = parse(minor_part)?;
    let newline = buf[11];
    if newline != b'\n' && newline != 0 {
        return Err(RfbError::Protocol(format!(
            "version line ended with {newline:#04x}, expected a newline"
        )));
    }
    Ok(major * 100 + minor)
}

/// Decide what to do about a version the client announced.
pub fn decide_version(client: u16) -> Result<VersionDecision, RfbError> {
    if client < 303 {
        return Err(RfbError::Protocol(format!(
            "client speaks RFB {}.{}, which is older than 3.3; there is no common \
             security handshake this server implements",
            client / 100,
            client % 100
        )));
    }
    if client > HIGHEST_SUPPORTED_VERSION {
        return Ok(VersionDecision::Retry);
    }
    if !SUPPORTED_VERSIONS.contains(&client) {
        // 3.4 - 3.6 are hypothetical. Accepting a dialect that was never
        // implemented is worse than saying so.
        return Err(RfbError::Protocol(format!(
            "client speaks RFB 3.{}, which this server does not implement; \
             supported versions are 3.3, 3.7 and 3.8",
            client % 100
        )));
    }
    Ok(VersionDecision::Accept(client))
}

/// The security types this server is willing to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityType(pub u8);

/// Pick the security type to run, given what this server offers and what the
/// client offered.
///
/// The server chooses; RFC 6143 is explicit about that. Preference order is
/// VNC Auth over None so that a client advertising both cannot talk the server
/// down into an unauthenticated session — a downgrade an attacker who can
/// inject into the handshake should not be able to win.
pub fn negotiate_security_type(
    client_types: &[u8],
    allow_none: bool,
) -> Result<SecurityType, RfbError> {
    if !allow_none && client_types.contains(&SECURITY_NONE) && !client_types.contains(&SECURITY_VNC_AUTH)
    {
        return Err(RfbError::Protocol(
            "client offered only unauthenticated access, which this server refuses; \
             it requires VNC Authentication"
                .into(),
        ));
    }
    if client_types.contains(&SECURITY_VNC_AUTH) {
        return Ok(SecurityType(SECURITY_VNC_AUTH));
    }
    if allow_none && client_types.contains(&SECURITY_NONE) {
        return Ok(SecurityType(SECURITY_NONE));
    }
    Err(RfbError::Protocol(format!(
        "no acceptable security type; client offered {:?} and this server requires VNC \
         Authentication",
        describe_security_types(client_types)
    )))
}

/// Render a security type list for an error message, naming what we recognise.
fn describe_security_types(types: &[u8]) -> String {
    let mut parts = Vec::with_capacity(types.len());
    for t in types {
        parts.push(match *t {
            SECURITY_NONE => "None(1)".to_string(),
            SECURITY_VNC_AUTH => "VNCAuth(2)".to_string(),
            16 => "Tight(16)".to_string(),
            19 => "VeNCrypt(19)".to_string(),
            30 => "AppleVNCAuth(30)".to_string(),
            other => other.to_string(),
        });
    }
    parts.join(", ")
}

/// The security types to advertise, given whether None is allowed.
pub fn security_type_offer(allow_none: bool) -> Vec<u8> {
    if allow_none {
        vec![SECURITY_NONE, SECURITY_VNC_AUTH]
    } else {
        vec![SECURITY_VNC_AUTH]
    }
}

/// One pixel layout, as RFB describes it.
///
/// Sixteen bytes on the wire. The three `(max, shift)` pairs are how a client is
/// told where a colour lives inside a pixel: `value = component & max` sits at
/// `shift`, and the client scales it to 8 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// 32 bpp, 24 significant bits, little-endian: `B G R x` in memory.
///
/// The x byte is discarded. Sending it costs nothing on the wire and every
/// client, viewer and hardware blitter since 1998 already handles it, whereas
/// dropping it means a fourth format to get right.
impl PixelFormat {
    pub const BGRX32: PixelFormat = PixelFormat {
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

    /// 16 bpp, 15 significant bits: packed RGB 5-5-5, little-endian.
    ///
    /// Half the bandwidth of BGRX32. Lossy in the sense that it quantises each
    /// channel to 5 bits, which on a dark desktop shows as banding; it is here
    /// because a client that asks for it over a slow link would otherwise have
    /// to be refused.
    pub const RGB555: PixelFormat = PixelFormat {
        bits_per_pixel: 16,
        depth: 15,
        big_endian: false,
        true_colour: true,
        red_max: 31,
        green_max: 31,
        blue_max: 31,
        red_shift: 10,
        green_shift: 5,
        blue_shift: 0,
    };

    /// Bytes one pixel occupies.
    pub const fn bytes_per_pixel(&self) -> usize {
        (self.bits_per_pixel as usize).div_ceil(8)
    }

    /// Reject a format this server cannot actually produce.
    ///
    /// A client is allowed to set almost any pixel format; the server is not
    /// obliged to honour it, and honouring a format it cannot encode means
    /// sending the wrong colours, which looks like a rendering bug rather than
    /// a protocol error.
    pub fn validate(&self) -> Result<(), RfbError> {
        let supported = matches!(self.bits_per_pixel, 8 | 16 | 32);
        if !supported {
            return Err(RfbError::Protocol(format!(
                "{}-bit pixels are not supported; this server offers 32bpp BGRA and 16bpp 555",
                self.bits_per_pixel
            )));
        }
        if !self.true_colour {
            return Err(RfbError::Protocol(
                "palette (true-colour=false) pixel formats are not supported".into(),
            ));
        }
        if self.depth == 0 || self.depth > self.bits_per_pixel {
            return Err(RfbError::Protocol(format!(
                "depth {} is inconsistent with {}-bit pixels",
                self.depth, self.bits_per_pixel
            )));
        }
        if self.red_max == 0 || self.green_max == 0 || self.blue_max == 0 {
            return Err(RfbError::Protocol(
                "a true-colour format with a zero component maximum cannot be encoded".into(),
            ));
        }
        // A component must fit inside its pixel. Checking the *shifted* width
        // catches a format that would silently spill into the neighbouring
        // channel, which corrupts colours rather than failing.
        for (name, max, shift) in [
            ("red", self.red_max, self.red_shift),
            ("green", self.green_max, self.green_shift),
            ("blue", self.blue_max, self.blue_shift),
        ] {
            let used = component_bits(max) + u32::from(shift);
            if used > self.bits_per_pixel as u32 {
                return Err(RfbError::Protocol(format!(
                    "{name} component (max {max}, shift {shift}) needs {used} bits but a \
                     pixel is only {} bits",
                    self.bits_per_pixel
                )));
            }
        }
        Ok(())
    }

    /// Bytes one row of `width` pixels occupies.
    pub fn row_stride(&self, width: u16) -> usize {
        width as usize * self.bytes_per_pixel()
    }

    /// Whether a Tight `TPIXEL` is a three-byte R, G, B triple for this format.
    ///
    /// The Tight specification defines `TPIXEL` as identical to `PIXEL` *except*
    /// when the format is 32bpp with depth 24 and exactly 8 bits in each of the
    /// three components — the very common BGRX case — where it is three bytes in
    /// red, green, blue order. Sending four-byte `PIXEL`s there instead makes a
    /// Tight client render every rectangle with its channels rotated, so this
    /// predicate is what separates the two code paths.
    pub fn tpixel_is_rgb_triple(&self) -> bool {
        self.true_colour
            && self.bits_per_pixel == 32
            && self.depth == 24
            && self.red_max == 255
            && self.green_max == 255
            && self.blue_max == 255
    }

    /// Pack one BGRX32 source pixel into this format.
    ///
    /// `bgra` is the server's canonical layout; the returned array is zero-padded
    /// past `bytes_per_pixel()` so callers can index it without a bounds check
    /// and `write_pixel` can copy a prefix out of it.
    pub fn encode_pixel(&self, bgra: [u8; 4]) -> [u8; 4] {
        let mut out = [0u8; 4];
        let bpp = self.bytes_per_pixel();
        let components = [
            (bgra[0], self.blue_shift, self.blue_max),
            (bgra[1], self.green_shift, self.green_max),
            (bgra[2], self.red_shift, self.red_max),
        ];
        for (value, shift, max) in components {
            let word = (scale_component(value, max) as u32) << shift;
            for (i, byte) in word.to_le_bytes().iter().enumerate().take(bpp) {
                out[i] |= byte;
            }
        }
        if self.big_endian {
            out[..bpp].reverse();
        }
        if bpp == 4 {
            // The fourth byte of a 32bpp pixel is outside `depth` for the common
            // 24-bit formats, and the specification leaves it undefined. Carrying
            // the source alpha through anyway keeps the stream self-consistent:
            // Hextile restates a whole tile as one pixel, so a zero here would
            // silently drop the alpha of every flat region.
            out[3] = bgra[3];
        }
        out
    }

    /// Append one BGRX32 pixel to `out` in this format.
    ///
    /// Takes a `Vec` rather than a slice because slicing `out[..n]` would
    /// overwrite bytes already written instead of appending, which silently
    /// produces a truncated pixel stream rather than an error.
    pub fn write_pixel(&self, bgra: [u8; 4], out: &mut Vec<u8>) {
        let encoded = self.encode_pixel(bgra);
        out.extend_from_slice(&encoded[..self.bytes_per_pixel()]);
    }
}

/// Number of significant bits in a component maximum.
fn component_bits(max: u16) -> u32 {
    if max == 0 {
        0
    } else {
        16 - max.leading_zeros()
    }
}

/// Scale an 8-bit component onto a smaller component range.
///
/// Multiplication by `max`, not a shift: a 5-bit range is 0..=31, so `255 * 31`
/// overflows a byte and the division has to happen in a wider type or the top
/// half of the range comes out wrong.
fn scale_component(value: u8, max: u16) -> u16 {
    ((value as u32 * max as u32 + 127) / 255) as u16
}

/// The framebuffer geometry and name a client is told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInit {
    pub width: u16,
    pub height: u16,
    pub pixel_format: PixelFormat,
    pub name: String,
}

impl ServerInit {
    pub fn encode(&self) -> Vec<u8> {
        let name = self.name.as_bytes();
        let name = &name[..name.len().min(MAX_DESKTOP_NAME)];
        let mut out = Vec::with_capacity(24 + name.len());
        out.extend_from_slice(&self.width.to_be_bytes());
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.pixel_format.encode());
        out.extend_from_slice(&(name.len() as u32).to_be_bytes());
        out.extend_from_slice(name);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, RfbError> {
        if buf.len() < 24 {
            return Err(RfbError::Protocol("ServerInit was truncated".into()));
        }
        let width = u16::from_be_bytes([buf[0], buf[1]]);
        let height = u16::from_be_bytes([buf[2], buf[3]]);
        let pixel_format = PixelFormat::decode(&buf[4..20])?;
        let name_len = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]) as usize;
        let end = 24usize
            .checked_add(name_len)
            .ok_or_else(|| RfbError::Protocol("ServerInit name length overflowed".into()))?;
        if end > buf.len() {
            return Err(RfbError::Protocol("ServerInit name was truncated".into()));
        }
        let name = String::from_utf8_lossy(&buf[24..end]).into_owned();
        Ok(ServerInit {
            width,
            height,
            pixel_format,
            name,
        })
    }
}

impl PixelFormat {
    pub fn encode(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0] = self.bits_per_pixel;
        out[1] = self.depth;
        out[2] = u8::from(self.big_endian);
        out[3] = u8::from(self.true_colour);
        out[4..6].copy_from_slice(&self.red_max.to_be_bytes());
        out[6..8].copy_from_slice(&self.green_max.to_be_bytes());
        out[8..10].copy_from_slice(&self.blue_max.to_be_bytes());
        out[10] = self.red_shift;
        out[11] = self.green_shift;
        out[12] = self.blue_shift;
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, RfbError> {
        if buf.len() < 16 {
            return Err(RfbError::Protocol("pixel format was truncated".into()));
        }
        Ok(PixelFormat {
            bits_per_pixel: buf[0],
            depth: buf[1],
            big_endian: buf[2] != 0,
            true_colour: buf[3] != 0,
            red_max: u16::from_be_bytes([buf[4], buf[5]]),
            green_max: u16::from_be_bytes([buf[6], buf[7]]),
            blue_max: u16::from_be_bytes([buf[8], buf[9]]),
            red_shift: buf[10],
            green_shift: buf[11],
            blue_shift: buf[12],
        })
    }
}

/// One rectangle inside a `FramebufferUpdate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub encoding: i32,
}

impl Rect {
    /// A rectangle covering no pixels. Never sent: a zero-area rect is a way
    /// to make a client allocate for nothing.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn pixel_count(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    pub fn area(&self, format: &PixelFormat) -> usize {
        self.pixel_count() as usize * format.bytes_per_pixel()
    }

    pub fn write_header(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.y.to_be_bytes());
        out.extend_from_slice(&self.width.to_be_bytes());
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.encoding.to_be_bytes());
    }
}

/// Move a block of pixels within a framebuffer, without sending the pixels.
///
/// Only meaningful when the block's new contents are identical to its old
/// contents at the source offset, which is exactly what a scroll produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyRect {
    pub dst_x: u16,
    pub dst_y: u16,
    pub width: u16,
    pub height: u16,
    pub src_x: u16,
    pub src_y: u16,
}

/// A decoded client-to-server message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    SetPixelFormat(PixelFormat),
    SetEncodings {
        encodings: Vec<i32>,
    },
    FramebufferUpdateRequest {
        incremental: bool,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    KeyEvent {
        down: bool,
        keysym: u32,
    },
    PointerEvent {
        x: u16,
        y: u16,
        mask: u8,
    },
    ClientCutText {
        text: String,
    },
    /// A message type this server does not implement, with its bytes already
    /// consumed. RFC 6143 requires it to be ignored rather than treated as a
    /// protocol error: a client one version ahead must not lose its session.
    Ignored {
        message_type: u8,
    },
}

impl ClientMessage {
    pub fn message_type(&self) -> u8 {
        match self {
            ClientMessage::SetPixelFormat(_) => client_messages::SET_PIXEL_FORMAT,
            ClientMessage::SetEncodings { .. } => client_messages::SET_ENCODINGS,
            ClientMessage::FramebufferUpdateRequest { .. } => {
                client_messages::FRAMEBUFFER_UPDATE_REQUEST
            }
            ClientMessage::KeyEvent { .. } => client_messages::KEY_EVENT,
            ClientMessage::PointerEvent { .. } => client_messages::POINTER_EVENT,
            ClientMessage::ClientCutText { .. } => client_messages::CLIENT_CUT_TEXT,
            ClientMessage::Ignored { message_type } => *message_type,
        }
    }
}

/// Longest fixed-size client message. `SetPixelFormat` is 20 bytes and is the
/// longest thing a client sends; anything larger is read with an explicit
/// length rather than into a fixed buffer.
pub const CLIENT_MESSAGE_READ_LIMIT: usize = 64;

/// Decode one client message from the front of `buf`.
///
/// Returns the message and how many bytes it consumed. Every length is bounds
/// checked against `buf`, so a truncated or hostile frame is an `Err` rather
/// than a panic.
pub fn decode_client_message(buf: &[u8]) -> Result<(ClientMessage, usize), RfbError> {
    let Some(&message_type) = buf.first() else {
        return Err(RfbError::Protocol("empty client message".into()));
    };
    let need = |n: usize| -> Result<(), RfbError> {
        if buf.len() < n {
            Err(RfbError::Protocol(format!(
                "client message {message_type} was truncated: have {} bytes, need {n}",
                buf.len()
            )))
        } else {
            Ok(())
        }
    };

    match message_type {
        client_messages::SET_PIXEL_FORMAT => {
            // 1 type byte, 3 padding bytes, then the 16-byte format.
            need(20)?;
            Ok((
                ClientMessage::SetPixelFormat(PixelFormat::decode(&buf[4..20])?),
                20,
            ))
        }
        client_messages::SET_ENCODINGS => {
            need(4)?;
            let count = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            if count > MAX_CLIENT_ENCODINGS {
                return Err(RfbError::Protocol(format!(
                    "client advertised {count} encodings; this server accepts at most \
                     {MAX_CLIENT_ENCODINGS}"
                )));
            }
            let end = 4 + count * 4;
            need(end)?;
            let encodings = buf[4..end]
                .chunks_exact(4)
                .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            Ok((ClientMessage::SetEncodings { encodings }, end))
        }
        client_messages::FRAMEBUFFER_UPDATE_REQUEST => {
            need(10)?;
            Ok((
                ClientMessage::FramebufferUpdateRequest {
                    incremental: buf[1] != 0,
                    x: u16::from_be_bytes([buf[2], buf[3]]),
                    y: u16::from_be_bytes([buf[4], buf[5]]),
                    width: u16::from_be_bytes([buf[6], buf[7]]),
                    height: u16::from_be_bytes([buf[8], buf[9]]),
                },
                10,
            ))
        }
        client_messages::KEY_EVENT => {
            need(8)?;
            Ok((
                ClientMessage::KeyEvent {
                    down: buf[1] != 0,
                    keysym: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
                },
                8,
            ))
        }
        client_messages::POINTER_EVENT => {
            need(6)?;
            Ok((
                ClientMessage::PointerEvent {
                    x: u16::from_be_bytes([buf[2], buf[3]]),
                    y: u16::from_be_bytes([buf[4], buf[5]]),
                    mask: buf[1],
                },
                6,
            ))
        }
        client_messages::CLIENT_CUT_TEXT => {
            need(8)?;
            let len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
            if len > MAX_CUT_TEXT_BYTES {
                return Err(RfbError::Protocol(format!(
                    "ClientCutText claimed {len} bytes; this server accepts at most \
                     {MAX_CUT_TEXT_BYTES}"
                )));
            }
            let end = 8 + len;
            need(end)?;
            // Lossy: the field is nominally Latin-1, and a guest clipboard
            // holding UTF-8 should not be able to fail the whole message.
            let text = String::from_utf8_lossy(&buf[8..end]).into_owned();
            Ok((ClientMessage::ClientCutText { text }, end))
        }
        other => Err(RfbError::IgnoredMessage(other)),
    }
}

/// Append a `FramebufferUpdate` header for `rect_count` rectangles.
pub fn write_framebuffer_update_header(out: &mut Vec<u8>, rect_count: u16) {
    out.push(server_messages::FRAMEBUFFER_UPDATE);
    out.push(0); // padding
    out.extend_from_slice(&rect_count.to_be_bytes());
}

/// Append a `SecurityResult`. The reason string is only sent on failure, and
/// only on 3.8.
///
/// The `reason` is deliberately ignored when `ok` is true. Sending it anyway
/// appends a length-prefixed string to a *successful* result, and every client
/// then reads those bytes as the start of `ServerInit` — a stream desync that
/// presents as a server that authenticates nobody and then says nothing.
pub fn write_security_result(out: &mut Vec<u8>, ok: bool, reason: Option<&str>) {
    let code: u32 = if ok { 0 } else { 1 };
    out.extend_from_slice(&code.to_be_bytes());
    if ok {
        return;
    }
    if let Some(reason) = reason {
        let bytes = reason.as_bytes();
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_banner_is_exactly_twelve_bytes() {
        assert_eq!(SERVER_VERSION_BANNER.len(), 12);
        assert_eq!(&SERVER_VERSION_BANNER[..], b"RFB 003.008\n");
    }

    #[test]
    fn parses_the_three_supported_versions() {
        for (text, expected) in [
            (&b"RFB 003.003\n"[..], 303),
            (&b"RFB 003.007\n"[..], 307),
            (&b"RFB 003.008\n"[..], 308),
        ] {
            assert_eq!(parse_client_version(text).unwrap(), expected, "for {text:?}");
        }
        // The encoding is major * 100 + minor, which is what makes 4.x
        // unambiguous and matches every existing VNC implementation.
        assert_eq!(parse_client_version(b"RFB 004.001\n").unwrap(), 401);
    }

    #[test]
    fn version_parsing_rejects_garbage() {
        for bad in [
            &b"RFB 003.008"[..],           // too short
            &b"HTTP/1.1 200 OK\n\n"[..],   // not RFB at all
            &b"RFB abc.defg\n"[..],        // not digits
            &b"RFB 003.008X"[..],          // no newline
            &b"003.008\nxxxx"[..],         // no prefix
        ] {
            assert!(
                parse_client_version(bad).is_err(),
                "{:?} should not parse as a version",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn version_negotiation_accepts_33_37_and_38() {
        for v in [303u16, 307, 308] {
            assert_eq!(
                decide_version(v).unwrap(),
                VersionDecision::Accept(v),
                "3.{v} should be accepted"
            );
        }
    }

    #[test]
    fn version_negotiation_asks_newer_clients_to_retry() {
        for v in [309u16, 310, 400] {
            assert_eq!(
                decide_version(v).unwrap(),
                VersionDecision::Retry,
                "RFB {v} should get the banner again"
            );
        }
    }

    #[test]
    fn version_negotiation_refuses_versions_it_cannot_speak() {
        // Too old: there is no common security handshake.
        assert!(decide_version(302).is_err());
        // Hypothetical middle dialects: refusing beats speaking a dialect that
        // was never implemented.
        assert!(decide_version(304).is_err());
        assert!(decide_version(306).is_err());
    }

    #[test]
    fn security_offer_defaults_to_vnc_auth_only() {
        assert_eq!(security_type_offer(false), vec![SECURITY_VNC_AUTH]);
        assert_eq!(
            security_type_offer(true),
            vec![SECURITY_NONE, SECURITY_VNC_AUTH]
        );
    }

    #[test]
    fn security_negotiation_prefers_vnc_auth_over_none() {
        // The dangerous case is a client offering both: a downgrade must not be
        // something a man in the middle can talk the server into.
        assert_eq!(
            negotiate_security_type(&[SECURITY_NONE, SECURITY_VNC_AUTH], false).unwrap(),
            SecurityType(SECURITY_VNC_AUTH)
        );
        assert_eq!(
            negotiate_security_type(&[SECURITY_VNC_AUTH, SECURITY_NONE], true).unwrap(),
            SecurityType(SECURITY_VNC_AUTH)
        );
    }

    #[test]
    fn security_negotiation_allows_none_only_when_configured() {
        assert_eq!(
            negotiate_security_type(&[SECURITY_NONE, SECURITY_VNC_AUTH], true).unwrap(),
            SecurityType(SECURITY_VNC_AUTH)
        );
        assert_eq!(
            negotiate_security_type(&[SECURITY_NONE], true).unwrap(),
            SecurityType(SECURITY_NONE)
        );
        assert!(
            negotiate_security_type(&[SECURITY_NONE], false).is_err(),
            "None alone must be refused when unauthenticated access is not allowed"
        );
    }

    #[test]
    fn security_negotiation_rejects_clients_with_nothing_in_common() {
        // VeNCrypt, Tight and AppleVNCAuth are all real; none is implemented.
        assert!(negotiate_security_type(&[16, 19, 30], false).is_err());
        assert!(negotiate_security_type(&[], false).is_err());
        let err = negotiate_security_type(&[19], false).unwrap_err().to_string();
        assert!(err.contains("VeNCrypt(19)"), "error should list what was offered: {err}");
    }

    #[test]
    fn security_negotiation_rejects_a_plain_none_only_client_by_default() {
        // A client that offers only None against a server that requires auth.
        assert!(negotiate_security_type(&[SECURITY_NONE], false).is_err());
    }

    #[test]
    fn pixel_formats_round_trip_through_the_wire() {
        for format in [PixelFormat::BGRX32, PixelFormat::RGB555] {
            let bytes = format.encode();
            assert_eq!(bytes.len(), 16, "PixelFormat is exactly 16 bytes on the wire");
            assert_eq!(PixelFormat::decode(&bytes).unwrap(), format);
        }
    }

    #[test]
    fn server_init_round_trips_through_the_wire() {
        let init = ServerInit {
            width: 1280,
            height: 800,
            pixel_format: PixelFormat::BGRX32,
            name: "win11".into(),
        };
        assert_eq!(ServerInit::decode(&init.encode()).unwrap(), init);
        assert_eq!(init.encode().len(), 24 + 5);
    }

    #[test]
    fn server_init_rejects_a_truncated_body() {
        let init = ServerInit {
            width: 4,
            height: 4,
            pixel_format: PixelFormat::BGRX32,
            name: "vm".into(),
        };
        let bytes = init.encode();
        for cut in 0..bytes.len() {
            assert!(
                ServerInit::decode(&bytes[..cut]).is_err(),
                "a {cut}-byte prefix should not decode"
            );
        }
        assert!(ServerInit::decode(&bytes).is_ok());
    }

    #[test]
    fn bgrx32_puts_blue_first_in_memory() {
        let mut out: Vec<u8> = Vec::new();
        PixelFormat::BGRX32.write_pixel([0x11, 0x22, 0x33, 0xff], &mut out);
        // 0x11 is blue, and BGRX means blue lands in byte 0.
        assert_eq!(out, vec![0x11, 0x22, 0x33, 0xff], "the fourth byte carries alpha");
    }

    #[test]
    fn rgb555_packs_five_five_five_little_endian() {
        let mut out: Vec<u8> = Vec::new();
        // Pure red: red_max = 31 at shift 10, so the pixel value is 31 << 10.
        PixelFormat::RGB555.write_pixel([0x00, 0x00, 0xff, 0xff], &mut out);
        assert_eq!(out[0], 0x00, "low byte carries the green and blue bits");
        assert_eq!(out[1], 0x7C, "high byte carries the top five red bits");
        assert_eq!(u16::from_le_bytes([out[0], out[1]]), 0x7C00);
        // The two bytes past the pixel are untouched: 16bpp really is two bytes.
        assert_eq!(&out[2..], &[] as &[u8], "a 16bpp pixel is two bytes");
    }

    #[test]
    fn rgb555_channels_land_in_the_right_places() {
        // A fresh buffer per pixel: `write_pixel` appends, so reusing one would
        // read back the previous pixel's bytes.
        let mut blue: Vec<u8> = Vec::new();
        PixelFormat::RGB555.write_pixel([0xff, 0x00, 0x00, 0xff], &mut blue);
        assert_eq!(u16::from_le_bytes([blue[0], blue[1]]), 0x001F, "blue at shift 0");

        let mut green: Vec<u8> = Vec::new();
        PixelFormat::RGB555.write_pixel([0x00, 0xff, 0x00, 0xff], &mut green);
        assert_eq!(
            u16::from_le_bytes([green[0], green[1]]),
            0x03E0,
            "green at shift 5"
        );
    }

    #[test]
    fn rgb555_white_and_black_are_the_extremes() {
        let mut white: Vec<u8> = Vec::new();
        let mut black: Vec<u8> = Vec::new();
        PixelFormat::RGB555.write_pixel([0xff, 0xff, 0xff, 0xff], &mut white);
        PixelFormat::RGB555.write_pixel([0, 0, 0, 0xff], &mut black);
        assert_eq!(u16::from_le_bytes([white[0], white[1]]), 0b0111_1111_1111_1111);
        assert_eq!(u16::from_le_bytes([black[0], black[1]]), 0);
    }

    #[test]
    fn rgb555_quantisation_is_monotonic_and_spans_the_full_range() {
        // A 5-bit component must map 0..=255 onto 0..=31 with no gaps big enough
        // to show as banding. Shifting instead of scaling would produce 0, 8,
        // 16 ... and miss half the range entirely.
        let mut previous = 0u16;
        for value in 0..=255u8 {
            let mut out: Vec<u8> = Vec::new();
            PixelFormat::RGB555.write_pixel([value, value, value, 0xff], &mut out);
            let quantised = u16::from_le_bytes([out[0], out[1]]) & 0b1_1111;
            assert!(
                quantised >= previous,
                "quantisation went backwards at {value}: {quantised} after {previous}"
            );
            assert!(quantised <= 31, "{value} produced {quantised}, above the range");
            previous = quantised;
        }
        assert_eq!(previous, 31, "255 must reach the top of the 5-bit range");
    }

    #[test]
    fn rgb555_never_uses_the_top_bit_of_each_byte_as_a_component() {
        // 16bpp with depth 15 means bit 15 is unused. A format that put red at
        // shift 11 instead of 10 would set it, and a client honouring depth 15
        // would render the colour wrong.
        for value in 0..=255u8 {
            let mut out: Vec<u8> = Vec::new();
            PixelFormat::RGB555.write_pixel([value, value, value, 0xff], &mut out);
            assert_eq!(out[1] & 0x80, 0, "{value} set the padding bit");
        }
    }

    #[test]
    fn pixel_format_validation_rejects_what_cannot_be_produced() {
        assert!(PixelFormat::BGRX32.validate().is_ok());
        assert!(PixelFormat::RGB555.validate().is_ok());

        let mut palette = PixelFormat::BGRX32;
        palette.true_colour = false;
        assert!(palette.validate().is_err(), "palette formats are unsupported");

        let mut odd = PixelFormat::BGRX32;
        odd.bits_per_pixel = 24;
        assert!(odd.validate().is_err(), "24bpp is not offered");

        let mut deep = PixelFormat::BGRX32;
        deep.depth = 40;
        assert!(deep.validate().is_err(), "depth cannot exceed bpp");

        let mut zero = PixelFormat::BGRX32;
        zero.red_max = 0;
        assert!(zero.validate().is_err(), "a zero component max is unencodable");

        // A component that spills past the pixel width would corrupt the
        // neighbouring channel rather than failing.
        let mut spilling = PixelFormat::BGRX32;
        spilling.red_shift = 30;
        assert!(spilling.validate().is_err());
    }

    #[test]
    fn client_messages_round_trip() {
        let cases: Vec<(Vec<u8>, ClientMessage)> = vec![
            (
                vec![0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0],
                ClientMessage::SetPixelFormat(PixelFormat::BGRX32),
            ),
            (
                vec![2, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 5],
                ClientMessage::SetEncodings {
                    encodings: vec![0, 5],
                },
            ),
            (
                vec![3, 1, 0, 10, 0, 20, 0, 32, 0, 24],
                ClientMessage::FramebufferUpdateRequest {
                    incremental: true,
                    x: 10,
                    y: 20,
                    width: 32,
                    height: 24,
                },
            ),
            (
                vec![4, 1, 0, 0, 0, 0, 0, 0x61],
                ClientMessage::KeyEvent {
                    down: true,
                    keysym: 0x61,
                },
            ),
            (
                vec![5, 1, 0, 100, 0, 200],
                ClientMessage::PointerEvent {
                    x: 100,
                    y: 200,
                    mask: 1,
                },
            ),
            (
                vec![6, 0, 0, 0, 0, 0, 0, 2, b'h', b'i'],
                ClientMessage::ClientCutText {
                    text: "hi".into(),
                },
            ),
        ];
        for (bytes, expected) in cases {
            let (decoded, consumed) = decode_client_message(&bytes).unwrap();
            assert_eq!(decoded, expected, "for {bytes:?}");
            assert_eq!(consumed, bytes.len(), "consumed the whole message");
            assert_eq!(decoded.message_type(), bytes[0]);
        }
    }

    #[test]
    fn truncated_client_messages_are_errors_not_panics() {
        let full = vec![3, 1, 0, 10, 0, 20, 0, 32, 0, 24];
        assert_eq!(full.len(), 10, "a FramebufferUpdateRequest is exactly 10 bytes");
        for cut in 0..full.len() {
            assert!(
                decode_client_message(&full[..cut]).is_err(),
                "a {cut}-byte FramebufferUpdateRequest should not decode"
            );
        }
        assert!(decode_client_message(&full).is_ok());
    }

    #[test]
    fn an_unbounded_encoding_count_is_rejected_before_allocating() {
        // 65535 encodings is 256 KiB of client-controlled allocation.
        let hostile = vec![2, 0, 0xff, 0xff];
        assert!(decode_client_message(&hostile).is_err());
    }

    #[test]
    fn an_unbounded_cut_text_length_is_rejected_before_allocating() {
        let hostile = vec![6, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        let err = decode_client_message(&hostile).unwrap_err().to_string();
        assert!(err.contains("ClientCutText"), "got: {err}");
    }

    #[test]
    fn an_unknown_message_type_is_reported_as_ignored() {
        // RFC 6143 7.5: unknown message types must be ignored, not fatal.
        for t in [1u8, 7, 200, 255] {
            let bytes = vec![t, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            match decode_client_message(&bytes) {
                Err(RfbError::IgnoredMessage(mt)) => assert_eq!(mt, t),
                other => panic!("expected IgnoredMessage({t}), got {other:?}"),
            }
        }
    }

    #[test]
    fn security_result_encodes_ok_and_failures() {
        let mut ok = Vec::new();
        write_security_result(&mut ok, true, None);
        assert_eq!(ok, vec![0, 0, 0, 0]);

        let mut failed = Vec::new();
        write_security_result(&mut failed, false, Some("no"));
        assert_eq!(failed, vec![0, 0, 0, 1, 0, 0, 0, 2, b'n', b'o']);

        // 3.3 and 3.7 send no reason at all.
        let mut quiet = Vec::new();
        write_security_result(&mut quiet, false, None);
        assert_eq!(quiet, vec![0, 0, 0, 1]);
    }

    #[test]
    fn a_successful_security_result_carries_nothing_after_the_status() {
        // Sending a reason on success desynchronises the stream: the client reads
        // it as the head of `ServerInit` and then blocks forever waiting for a
        // framebuffer that was already consumed as ASCII.
        let mut ok = Vec::new();
        write_security_result(&mut ok, true, Some("authentication failed"));
        assert_eq!(
            ok,
            vec![0, 0, 0, 0],
            "a success must be exactly four bytes"
        );
    }

    #[test]
    fn rects_with_no_pixels_report_themselves_empty() {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
            encoding: 0,
        };
        assert!(rect.is_empty());
        assert_eq!(rect.pixel_count(), 0);
    }
}