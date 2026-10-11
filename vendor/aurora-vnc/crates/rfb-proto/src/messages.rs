//! Normal-phase messages, RFC 6143 sections 7.5 (client to server) and 7.6
//! (server to client). Rectangle payloads are left to the encoding layer:
//! this module decodes the FramebufferUpdate header and each RectHeader.

use crate::clipboard::ExtClipboard;
use crate::codec::{len_u32, DecodeError, Limits, PutBe, Reader};
use crate::pseudo::{read_screens, Screen};
use crate::types::{Encoding, PixelFormat};
use crate::video::{read_envelope, AuroraClientMessage, AuroraServerMessage, AURORA_MSG_TYPE};

/// Type 251 (rfbproto SetDesktopSize).
const SET_DESKTOP_SIZE: u8 = 251;

/// Writes a cut-text message carrying an Extended Clipboard body, whose
/// length goes on the wire negated.
fn put_ext_clipboard(msg_type: u8, m: &ExtClipboard, out: &mut Vec<u8>) {
    out.put_u8(msg_type);
    out.extend_from_slice(&[0, 0, 0]);
    let mut body = Vec::new();
    m.encode_body(&mut body);
    let len = i32::try_from(body.len()).expect("extended clipboard body longer than i32::MAX");
    out.put_i32(-len);
    out.extend_from_slice(&body);
}

/// Classic or extended cut text, after the type and padding. A negative
/// s32 length means an Extended Clipboard body of `-length` bytes; callers
/// should only act on that once the pseudo-encoding is negotiated.
fn read_cut_text(r: &mut Reader<'_>, limits: &Limits) -> Result<Result<Vec<u8>, ExtClipboard>, DecodeError> {
    let len = r.i32()?;
    if len >= 0 {
        let len = len as u32;
        if len > limits.max_cut_text_len {
            return Err(DecodeError::Invalid(format!(
                "cut text length {len} exceeds limit {}",
                limits.max_cut_text_len
            )));
        }
        return Ok(Ok(r.bytes(len as usize)?.to_vec()));
    }
    let n = len.checked_neg().ok_or_else(|| DecodeError::Invalid("extended clipboard length i32::MIN".into()))? as u32;
    let max = ExtClipboard::max_body_len(limits.max_cut_text_len);
    if n > max {
        return Err(DecodeError::Invalid(format!("extended clipboard length {n} exceeds limit {max}")));
    }
    let body = r.bytes(n as usize)?;
    Ok(Err(ExtClipboard::decode_body(body, limits.max_cut_text_len)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    SetPixelFormat(PixelFormat),
    SetEncodings(Vec<Encoding>),
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
        button_mask: u8,
        x: u16,
        y: u16,
    },
    ClientCutText(Vec<u8>),
    /// ClientCutText with a negative length (Extended Clipboard).
    ExtendedClipboard(ExtClipboard),
    /// Ask for a new framebuffer size and screen layout (ExtendedDesktopSize).
    SetDesktopSize {
        width: u16,
        height: u16,
        screens: Vec<Screen>,
    },
    /// Message type 65 (Aurora Video and later Aurora extensions). Only sent
    /// to a server that sent an Aurora message first.
    Aurora(AuroraClientMessage),
}

impl ClientMessage {
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            ClientMessage::SetPixelFormat(pf) => {
                out.put_u8(0);
                out.extend_from_slice(&[0, 0, 0]);
                pf.encode(out);
            }
            ClientMessage::SetEncodings(encs) => {
                out.put_u8(2);
                out.put_u8(0);
                let n = u16::try_from(encs.len()).expect("more than 65535 encodings");
                out.put_u16(n);
                for e in encs {
                    out.put_i32(e.code());
                }
            }
            ClientMessage::FramebufferUpdateRequest { incremental, x, y, width, height } => {
                out.put_u8(3);
                out.put_u8(*incremental as u8);
                out.put_u16(*x);
                out.put_u16(*y);
                out.put_u16(*width);
                out.put_u16(*height);
            }
            ClientMessage::KeyEvent { down, keysym } => {
                out.put_u8(4);
                out.put_u8(*down as u8);
                out.extend_from_slice(&[0, 0]);
                out.put_u32(*keysym);
            }
            ClientMessage::PointerEvent { button_mask, x, y } => {
                out.put_u8(5);
                out.put_u8(*button_mask);
                out.put_u16(*x);
                out.put_u16(*y);
            }
            ClientMessage::ClientCutText(text) => {
                out.put_u8(6);
                out.extend_from_slice(&[0, 0, 0]);
                out.put_u32(len_u32(text.len(), "cut text"));
                out.extend_from_slice(text);
            }
            ClientMessage::ExtendedClipboard(m) => put_ext_clipboard(6, m, out),
            ClientMessage::SetDesktopSize { width, height, screens } => {
                out.put_u8(SET_DESKTOP_SIZE);
                out.put_u8(0);
                out.put_u16(*width);
                out.put_u16(*height);
                out.put_u8(u8::try_from(screens.len()).expect("more than 255 screens"));
                out.put_u8(0);
                for s in screens {
                    s.encode(out);
                }
            }
            ClientMessage::Aurora(m) => m.encode(out),
        }
    }

    pub fn decode(buf: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let msg = match r.u8()? {
            0 => {
                r.skip(3)?;
                ClientMessage::SetPixelFormat(PixelFormat::read(&mut r)?)
            }
            2 => {
                r.skip(1)?;
                let n = r.u16()?;
                if n > limits.max_encodings {
                    return Err(DecodeError::Invalid(format!("{n} encodings exceeds limit")));
                }
                // Check the whole message is present before allocating.
                r.bytes(4 * n as usize)?;
                let mut r2 = Reader::new(&buf[4..]);
                let encs = (0..n).map(|_| r2.i32().map(Encoding::from_code)).collect::<Result<_, _>>()?;
                ClientMessage::SetEncodings(encs)
            }
            3 => ClientMessage::FramebufferUpdateRequest {
                incremental: r.u8()? != 0,
                x: r.u16()?,
                y: r.u16()?,
                width: r.u16()?,
                height: r.u16()?,
            },
            4 => {
                let down = r.u8()? != 0;
                r.skip(2)?;
                ClientMessage::KeyEvent { down, keysym: r.u32()? }
            }
            5 => ClientMessage::PointerEvent { button_mask: r.u8()?, x: r.u16()?, y: r.u16()? },
            6 => {
                r.skip(3)?;
                match read_cut_text(&mut r, limits)? {
                    Ok(text) => ClientMessage::ClientCutText(text),
                    Err(ext) => ClientMessage::ExtendedClipboard(ext),
                }
            }
            SET_DESKTOP_SIZE => {
                r.skip(1)?;
                let (width, height) = (r.u16()?, r.u16()?);
                let n = r.u8()?;
                r.skip(1)?;
                ClientMessage::SetDesktopSize { width, height, screens: read_screens(&mut r, n)? }
            }
            AURORA_MSG_TYPE => {
                let (sub, body) = read_envelope(&mut r, limits)?;
                ClientMessage::Aurora(AuroraClientMessage::decode_body(sub, body, limits)?)
            }
            t => return Err(DecodeError::Invalid(format!("unknown client message type {t}"))),
        };
        Ok((msg, r.consumed()))
    }
}

/// Server-to-client messages. For FramebufferUpdate only the header is
/// decoded; `num_rects` RectHeaders (each followed by its payload) come next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    FramebufferUpdate {
        num_rects: u16,
    },
    SetColourMapEntries {
        first_colour: u16,
        colours: Vec<[u16; 3]>,
    },
    Bell,
    ServerCutText(Vec<u8>),
    /// ServerCutText with a negative length (Extended Clipboard).
    ExtendedClipboard(ExtClipboard),
    /// Message type 65 (Aurora Video and later Aurora extensions). Only sent
    /// to a client that listed `Encoding::AuroraVideo`.
    Aurora(AuroraServerMessage),
}

impl ServerMessage {
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            ServerMessage::FramebufferUpdate { num_rects } => {
                out.put_u8(0);
                out.put_u8(0);
                out.put_u16(*num_rects);
            }
            ServerMessage::SetColourMapEntries { first_colour, colours } => {
                out.put_u8(1);
                out.put_u8(0);
                out.put_u16(*first_colour);
                out.put_u16(u16::try_from(colours.len()).expect("more than 65535 colours"));
                for [r, g, b] in colours {
                    out.put_u16(*r);
                    out.put_u16(*g);
                    out.put_u16(*b);
                }
            }
            ServerMessage::Bell => out.put_u8(2),
            ServerMessage::ServerCutText(text) => {
                out.put_u8(3);
                out.extend_from_slice(&[0, 0, 0]);
                out.put_u32(len_u32(text.len(), "cut text"));
                out.extend_from_slice(text);
            }
            ServerMessage::ExtendedClipboard(m) => put_ext_clipboard(3, m, out),
            ServerMessage::Aurora(m) => m.encode(out),
        }
    }

    pub fn decode(buf: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let msg = match r.u8()? {
            0 => {
                r.skip(1)?;
                ServerMessage::FramebufferUpdate { num_rects: r.u16()? }
            }
            1 => {
                r.skip(1)?;
                let first_colour = r.u16()?;
                let n = r.u16()?;
                if n > limits.max_colour_map_entries {
                    return Err(DecodeError::Invalid(format!("{n} colour map entries exceeds limit")));
                }
                r.bytes(6 * n as usize)?;
                let mut r2 = Reader::new(&buf[6..]);
                let colours =
                    (0..n).map(|_| Ok([r2.u16()?, r2.u16()?, r2.u16()?])).collect::<Result<_, DecodeError>>()?;
                ServerMessage::SetColourMapEntries { first_colour, colours }
            }
            2 => ServerMessage::Bell,
            3 => {
                r.skip(3)?;
                match read_cut_text(&mut r, limits)? {
                    Ok(text) => ServerMessage::ServerCutText(text),
                    Err(ext) => ServerMessage::ExtendedClipboard(ext),
                }
            }
            AURORA_MSG_TYPE => {
                let (sub, body) = read_envelope(&mut r, limits)?;
                ServerMessage::Aurora(AuroraServerMessage::decode_body(sub, body, limits)?)
            }
            t => return Err(DecodeError::Invalid(format!("unknown server message type {t}"))),
        };
        Ok((msg, r.consumed()))
    }
}

/// The 12-byte header that precedes each rectangle in a FramebufferUpdate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectHeader {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub encoding: Encoding,
}

impl RectHeader {
    pub const WIRE_LEN: usize = 12;

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u16(self.x);
        out.put_u16(self.y);
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.put_i32(self.encoding.code());
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(buf);
        let h = RectHeader {
            x: r.u16()?,
            y: r.u16()?,
            width: r.u16()?,
            height: r.u16()?,
            encoding: Encoding::from_code(r.i32()?),
        };
        Ok((h, r.consumed()))
    }

    /// Payload size in bytes for a Raw rectangle in `pf`, computed without overflow.
    pub fn raw_payload_len(&self, pf: &PixelFormat) -> usize {
        self.width as usize * self.height as usize * pf.bytes_per_pixel()
    }
}
