//! Sans-I/O RFB client session.
//!
//! ```text
//!   socket bytes --> Session::receive --> Vec<Event>
//!   key/pointer/... --> Session::{key, pointer, ...} --> Session::take_output --> socket
//! ```
//!
//! The session never blocks, allocates only what the server has already sent
//! (bounded by [`SessionOptions::max_framebuffer_pixels`]), and treats any
//! malformed or out-of-bounds data as a fatal [`ClientError`] rather than
//! guessing. Unknown server message types are fatal too, because RFB gives no
//! way to know their length; new features must be negotiated first.

use std::fmt;

use rfb_proto::{
    ClientConfig, ClientHandshake, ClientMessage, DecodeError, Encoding, HandshakeError, PixelFormat, RectHeader,
    ServerInit, ServerMessage,
};

use rfb_tls::{TlsInfo, TlsSession};

use crate::classic;
use crate::cursor::{self, CursorShape};
use crate::desktop;
use crate::framebuffer::{Framebuffer, Rect};
use crate::tight::{self, TightDecoder};
use crate::zrle::ZrleDecoder;

/// TLS settings for VeNCrypt. Required whenever the handshake may negotiate
/// VeNCrypt (e.g. with `ClientConfig::encrypted()`).
#[derive(Clone)]
pub struct TlsOptions {
    pub config: std::sync::Arc<rfb_tls::rustls::ClientConfig>,
    /// DNS name or IP address the certificate must match (unless pinned).
    pub server_name: String,
}

impl fmt::Debug for TlsOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsOptions").field("server_name", &self.server_name).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Handshake settings (version, security types, password, shared flag, limits).
    pub handshake: ClientConfig,
    /// TLS for VeNCrypt. When the server picks VeNCrypt and this is `None`,
    /// the session fails instead of continuing unencrypted.
    pub tls: Option<TlsOptions>,
    /// Pixel format to ask the server for. `None` keeps the server's native format.
    pub pixel_format: Option<PixelFormat>,
    /// Encodings to advertise, most preferred first. Only ones this crate decodes
    /// are sent; others are filtered out so the server can never use them.
    pub encodings: Vec<Encoding>,
    /// Ask for the whole screen as soon as the handshake completes.
    pub request_initial_frame: bool,
    /// After each complete update, automatically request the next incremental one.
    pub continuous_updates: bool,
    /// Refuse framebuffers larger than this many pixels (memory safety bound).
    pub max_framebuffer_pixels: u64,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            handshake: ClientConfig::default(),
            tls: None,
            pixel_format: Some(PixelFormat::RGBX8888_LE),
            encodings: rfb_proto::compat::advertise_encodings(&[
                Encoding::Hextile,
                Encoding::Rre,
                Encoding::ExtendedDesktopSize,
                Encoding::DesktopSize,
                Encoding::Cursor,
                Encoding::ExtendedClipboard,
            ]),
            request_initial_frame: true,
            continuous_updates: true,
            max_framebuffer_pixels: 16384 * 16384,
        }
    }
}

impl SessionOptions {
    /// Encrypted-only session: VeNCrypt with certificate-authenticated TLS 1.3.
    pub fn encrypted(tls: TlsOptions) -> Self {
        SessionOptions { handshake: ClientConfig::encrypted(), tls: Some(tls), ..Default::default() }
    }
}

/// Encodings this crate can decode today.
pub const SUPPORTED_ENCODINGS: &[Encoding] = &[
    Encoding::Raw,
    Encoding::CopyRect,
    Encoding::Rre,
    Encoding::Hextile,
    Encoding::Zrle,
    Encoding::Tight,
    Encoding::DesktopSize,
    Encoding::ExtendedDesktopSize,
    Encoding::Cursor,
    Encoding::ExtendedClipboard,
];

/// Tight JPEG quality pseudo-encoding (0 = smallest, 9 = best). Sending one
/// allows the server to use lossy JPEG for photo-like areas; without it,
/// Tight stays lossless.
pub fn jpeg_quality(level: u8) -> Encoding {
    Encoding::Other(-32 + i32::from(level.min(9)))
}

/// zlib compression level pseudo-encoding (0 = fastest, 9 = smallest).
pub fn compress_level(level: u8) -> Encoding {
    Encoding::Other(-256 + i32::from(level.min(9)))
}

fn is_supported(e: &Encoding) -> bool {
    SUPPORTED_ENCODINGS.contains(e) || matches!(e.code(), -32..=-23 | -256..=-247)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Handshake finished; the framebuffer now exists at this size.
    Connected {
        width: u16,
        height: u16,
        name: String,
    },
    /// One complete FramebufferUpdate was applied. `damage` lists changed areas.
    Updated {
        damage: Vec<Rect>,
    },
    /// The server changed the desktop size; the framebuffer was resized.
    Resized {
        width: u16,
        height: u16,
    },
    Bell,
    /// Clipboard text from a server without Extended Clipboard (Latin-1 per
    /// RFC 6143; [`crate::clipboard::from_latin1`] decodes it).
    ServerCutText(Vec<u8>),
    /// Clipboard text from a server with Extended Clipboard (UTF-8).
    ClipboardText(String),
    /// The server changed the pointer's shape (Cursor pseudo-encoding). Draw
    /// it at the local pointer position; a hidden shape means draw nothing.
    CursorShape(CursorShape),
    /// The server turned down our [`Session::request_desktop_size`]; the
    /// framebuffer is unchanged. `reason` is readable text.
    ResizeRefused {
        status: u16,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Handshake(HandshakeError),
    Tls(String),
    Protocol(String),
    /// The session already failed; create a new one to reconnect.
    Closed,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Handshake(e) => write!(f, "handshake: {e}"),
            ClientError::Tls(e) => write!(f, "TLS: {e}"),
            ClientError::Protocol(e) => write!(f, "protocol error: {e}"),
            ClientError::Closed => write!(f, "session closed after an earlier error"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<HandshakeError> for ClientError {
    fn from(e: HandshakeError) -> Self {
        ClientError::Handshake(e)
    }
}

fn proto(e: impl fmt::Display) -> ClientError {
    ClientError::Protocol(e.to_string())
}

enum Phase {
    Handshake(Box<ClientHandshake>),
    Normal,
    Failed,
}

pub struct Session {
    opts: SessionOptions,
    phase: Phase,
    /// Plaintext RFB bytes received but not yet consumed.
    inbuf: Vec<u8>,
    /// Plaintext RFB bytes queued for the server.
    out: Vec<u8>,
    /// Final bytes for the socket (already encrypted when TLS is active).
    wire_out: Vec<u8>,
    tls: Option<TlsSession>,
    server_init: Option<ServerInit>,
    pf: PixelFormat,
    colour_map: Vec<u32>,
    fb: Framebuffer,
    /// Rectangles still expected in the current FramebufferUpdate.
    rects_left: u16,
    damage: Vec<Rect>,
    buttons: u8,
    zrle: ZrleDecoder,
    tight: TightDecoder,
    rect_counts: Vec<(Encoding, u64)>,
    cursor: Option<CursorShape>,
    /// Screen layout from the last ExtendedDesktopSize; `None` until the
    /// server has sent one, which is also how we know it supports resizing.
    screens: Option<Vec<rfb_proto::Screen>>,
    clipboard: crate::clipboard::Clipboard,
    /// True once a pixel-carrying rect has been applied since the
    /// framebuffer was last created or resized (a resize clears it to black).
    painted: bool,
    /// A resize arrived without pixels; ask for a full refresh once.
    refresh_after_resize: bool,
}

impl Session {
    pub fn new(mut opts: SessionOptions) -> Self {
        opts.encodings.retain(is_supported);
        if !opts.encodings.contains(&Encoding::Raw) {
            opts.encodings.push(Encoding::Raw); // Raw is mandatory (RFC 6143 §7.7.1)
        }
        let hs = ClientHandshake::new(opts.handshake.clone());
        let clipboard = crate::clipboard::Clipboard::new(opts.handshake.limits.max_cut_text_len);
        Session {
            opts,
            phase: Phase::Handshake(Box::new(hs)),
            inbuf: Vec::new(),
            out: Vec::new(),
            wire_out: Vec::new(),
            tls: None,
            server_init: None,
            pf: PixelFormat::RGBX8888_LE,
            colour_map: Vec::new(),
            fb: Framebuffer::new(0, 0),
            rects_left: 0,
            damage: Vec::new(),
            buttons: 0,
            zrle: ZrleDecoder::default(),
            tight: TightDecoder::default(),
            rect_counts: Vec::new(),
            cursor: None,
            screens: None,
            clipboard,
            painted: false,
            refresh_after_resize: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Normal)
    }

    pub fn server_init(&self) -> Option<&ServerInit> {
        self.server_init.as_ref()
    }

    pub fn framebuffer(&self) -> &Framebuffer {
        &self.fb
    }

    /// The pixel format the server is currently sending in.
    pub fn pixel_format(&self) -> &PixelFormat {
        &self.pf
    }

    /// Negotiated TLS parameters, once the TLS handshake has completed.
    /// How many rectangles of `encoding` have been decoded (diagnostics, tests).
    pub fn rect_count(&self, encoding: Encoding) -> u64 {
        self.rect_counts.iter().find(|(e, _)| *e == encoding).map_or(0, |(_, n)| *n)
    }

    /// How many Tight rectangles arrived as lossy JPEG.
    /// The server's screen layout, once it has sent ExtendedDesktopSize.
    pub fn screens(&self) -> Option<&[rfb_proto::Screen]> {
        self.screens.as_deref()
    }

    /// True once the server has shown it understands client resize requests.
    pub fn can_resize(&self) -> bool {
        self.screens.is_some()
    }

    /// Asks the server to resize the remote desktop to `width x height` as a
    /// single screen. Returns false (and sends nothing) if the server hasn't
    /// offered ExtendedDesktopSize, the size is empty or over our limit, or
    /// it is already that size. The answer arrives as [`Event::Resized`] or
    /// [`Event::ResizeRefused`].
    pub fn request_desktop_size(&mut self, width: u16, height: u16) -> bool {
        let Some(current) = &self.screens else { return false };
        if !self.is_connected()
            || width == 0
            || height == 0
            || (width, height) == (self.fb.width(), self.fb.height())
            || self.check_size(width, height).is_err()
        {
            return false;
        }
        let screens = desktop::single_screen(current, width, height);
        ClientMessage::SetDesktopSize { width, height, screens }.encode(&mut self.out);
        true
    }

    /// The latest pointer shape from the server, if it has sent one.
    /// True once the server has sent real pixels for the current
    /// framebuffer size. An update carrying only a size change or a cursor
    /// shape (TightVNC's first one does) leaves this false.
    pub fn has_pixels(&self) -> bool {
        self.painted
    }

    pub fn cursor(&self) -> Option<&CursorShape> {
        self.cursor.as_ref()
    }

    pub fn jpeg_rect_count(&self) -> u64 {
        self.tight.jpeg_rects()
    }

    pub fn tls_info(&self) -> Option<TlsInfo> {
        self.tls.as_ref().and_then(|t| t.info())
    }

    /// True once traffic is going through TLS (handshake may still be running).
    pub fn is_encrypted(&self) -> bool {
        self.tls.is_some()
    }

    /// Bytes to write to the socket. Call after every `receive` and input call.
    pub fn take_output(&mut self) -> Vec<u8> {
        if let Phase::Handshake(hs) = &mut self.phase {
            self.out.extend(hs.take_output());
        }
        let plain = std::mem::take(&mut self.out);
        match &mut self.tls {
            Some(tls) => {
                if !plain.is_empty() && tls.send(&plain).is_err() {
                    self.phase = Phase::Failed;
                }
                self.wire_out.extend(tls.take_output());
            }
            None => self.wire_out.extend(plain),
        }
        std::mem::take(&mut self.wire_out)
    }

    /// Feed bytes read from the server. Returns every event they completed.
    pub fn receive(&mut self, input: &[u8]) -> Result<Vec<Event>, ClientError> {
        if matches!(self.phase, Phase::Failed) {
            return Err(ClientError::Closed);
        }
        let mut events = Vec::new();
        let res = self.feed_wire(input).and_then(|()| self.pump(&mut events));
        if res.is_err() {
            self.phase = Phase::Failed;
            self.inbuf.clear();
        }
        res.map(|()| events)
    }

    /// Socket bytes in: decrypt if TLS is up, then queue as RFB plaintext.
    fn feed_wire(&mut self, input: &[u8]) -> Result<(), ClientError> {
        match &mut self.tls {
            Some(tls) => {
                let plain = tls.receive(input).map_err(|e| ClientError::Tls(e.to_string()))?;
                self.inbuf.extend_from_slice(&plain);
                if tls.peer_closed() && plain.is_empty() && self.inbuf.is_empty() {
                    return Err(ClientError::Tls("server closed the TLS session".into()));
                }
            }
            None => self.inbuf.extend_from_slice(input),
        }
        Ok(())
    }

    /// The handshake asked for TLS: flush what it queued in the clear, start
    /// TLS, and re-feed any bytes already read as TLS ciphertext.
    fn start_tls(&mut self, hs_out: Vec<u8>, leftover_ciphertext: Vec<u8>) -> Result<(), ClientError> {
        let opts = self
            .opts
            .tls
            .clone()
            .ok_or_else(|| ClientError::Tls("server chose VeNCrypt but no TLS settings were given".into()))?;
        self.wire_out.extend(std::mem::take(&mut self.out));
        self.wire_out.extend(hs_out);
        let mut tls =
            TlsSession::client(opts.config, &opts.server_name).map_err(|e| ClientError::Tls(e.to_string()))?;
        self.wire_out.extend(tls.take_output()); // ClientHello
        self.tls = Some(tls);
        self.feed_wire(&leftover_ciphertext)
    }

    fn pump(&mut self, events: &mut Vec<Event>) -> Result<(), ClientError> {
        loop {
            let buf = std::mem::take(&mut self.inbuf);
            let mut pos = 0;
            let result = self.pump_buf(&buf, &mut pos, events);
            match result {
                Ok(true) => {
                    // Switched to TLS mid-buffer: the rest is ciphertext.
                    let rest = buf[pos..].to_vec();
                    let Phase::Handshake(hs) = &mut self.phase else { unreachable!() };
                    let hs_out = hs.take_output();
                    hs.tls_established()?;
                    self.start_tls(hs_out, rest)?;
                }
                other => {
                    self.inbuf = buf;
                    self.inbuf.drain(..pos);
                    return other.map(|_| ());
                }
            }
        }
    }

    /// Returns `Ok(true)` when the handshake paused for TLS at `*pos`.
    fn pump_buf(&mut self, buf: &[u8], pos: &mut usize, events: &mut Vec<Event>) -> Result<bool, ClientError> {
        if let Phase::Handshake(hs) = &mut self.phase {
            loop {
                if hs.needs_tls().is_some() {
                    if self.tls.is_some() {
                        return Err(ClientError::Tls("server asked to start TLS twice".into()));
                    }
                    return Ok(true);
                }
                if hs.is_done() {
                    break;
                }
                let used = hs.receive(&buf[*pos..])?;
                *pos += used;
                if used == 0 && hs.needs_tls().is_none() {
                    break;
                }
            }
            if !hs.is_done() {
                return Ok(false);
            }
            self.out.extend(hs.take_output());
            let si = hs.server_init().cloned().ok_or_else(|| proto("handshake done without ServerInit"))?;
            self.phase = Phase::Normal;
            self.on_connected(si, events)?;
        }
        while let Some(used) = self.step(&buf[*pos..], events)? {
            *pos += used;
        }
        Ok(false)
    }

    fn on_connected(&mut self, si: ServerInit, events: &mut Vec<Event>) -> Result<(), ClientError> {
        self.check_size(si.width, si.height)?;
        self.pf = si.pixel_format;
        if let Some(pf) = self.opts.pixel_format {
            pf.validate().map_err(proto)?;
            ClientMessage::SetPixelFormat(pf).encode(&mut self.out);
            self.pf = pf;
        }
        ClientMessage::SetEncodings(self.opts.encodings.clone()).encode(&mut self.out);
        self.fb = Framebuffer::new(si.width, si.height);
        if self.opts.request_initial_frame {
            self.request_update(false);
        }
        events.push(Event::Connected {
            width: si.width,
            height: si.height,
            name: String::from_utf8_lossy(&si.name).into_owned(),
        });
        self.server_init = Some(si);
        Ok(())
    }

    fn check_size(&self, w: u16, h: u16) -> Result<(), ClientError> {
        if w as u64 * h as u64 > self.opts.max_framebuffer_pixels {
            return Err(proto(format!("framebuffer {w}x{h} exceeds the configured limit")));
        }
        Ok(())
    }

    /// Tries to consume one unit (a message header or one rectangle) from `buf`.
    /// Returns `Ok(None)` when more bytes are needed.
    fn step(&mut self, buf: &[u8], events: &mut Vec<Event>) -> Result<Option<usize>, ClientError> {
        if buf.is_empty() {
            return Ok(None);
        }
        if self.rects_left > 0 {
            return self.step_rect(buf, events);
        }
        let limits = &self.opts.handshake.limits;
        let (msg, used) = match ServerMessage::decode(buf, limits) {
            Ok(v) => v,
            Err(DecodeError::Incomplete { .. }) => return Ok(None),
            Err(e) => return Err(proto(e)),
        };
        match msg {
            ServerMessage::FramebufferUpdate { num_rects } => {
                self.rects_left = num_rects;
                self.damage.clear();
                if num_rects == 0 {
                    self.finish_update(events);
                }
            }
            ServerMessage::SetColourMapEntries { first_colour, colours } => {
                let end = first_colour as usize + colours.len();
                if end > self.colour_map.len() {
                    self.colour_map.resize(end, 0);
                }
                for (i, [r, g, b]) in colours.iter().enumerate() {
                    self.colour_map[first_colour as usize + i] =
                        ((*r as u32 >> 8) << 16) | ((*g as u32 >> 8) << 8) | (*b as u32 >> 8);
                }
            }
            ServerMessage::Bell => events.push(Event::Bell),
            ServerMessage::ServerCutText(t) => {
                self.clipboard.note_remote(&crate::clipboard::from_latin1(&t));
                events.push(Event::ServerCutText(t));
            }
            ServerMessage::ExtendedClipboard(m) => {
                let (replies, text) = self.clipboard.on_server(m);
                for r in replies {
                    ClientMessage::ExtendedClipboard(r).encode(&mut self.out);
                }
                events.extend(text.map(Event::ClipboardText));
            }
            // Aurora messages (type 65) only go to clients that list
            // Encoding::AuroraVideo, which this client doesn't yet; ignored.
            ServerMessage::Aurora(_) => {}
        }
        Ok(Some(used))
    }

    fn step_rect(&mut self, buf: &[u8], events: &mut Vec<Event>) -> Result<Option<usize>, ClientError> {
        let (h, hl) = match RectHeader::decode(buf) {
            Ok(v) => v,
            Err(DecodeError::Incomplete { .. }) => return Ok(None),
            Err(e) => return Err(proto(e)),
        };
        let r = Rect { x: h.x, y: h.y, width: h.width, height: h.height };
        let (fw, fh) = (self.fb.width(), self.fb.height());
        if !self.opts.encodings.contains(&h.encoding) {
            return Err(proto(format!("server used encoding {:?}, which was not negotiated", h.encoding)));
        }
        let payload = match h.encoding {
            Encoding::Raw => h.raw_payload_len(&self.pf),
            Encoding::CopyRect => 4,
            Encoding::Rre => match classic::rre_len(&buf[hl..], &self.pf).map_err(proto)? {
                Some(n) => n,
                None => return Ok(None),
            },
            Encoding::Hextile => {
                if !r.fits(fw, fh) {
                    return Err(proto(format!("Hextile rect {r:?} outside {fw}x{fh} framebuffer")));
                }
                match classic::hextile_len(&buf[hl..], &self.pf, h.width, h.height).map_err(proto)? {
                    Some(n) => n,
                    None => return Ok(None),
                }
            }
            Encoding::DesktopSize => 0,
            Encoding::Cursor => cursor::payload_len(&self.pf, h.width, h.height).map_err(proto)?,
            Encoding::ExtendedDesktopSize => match desktop::payload_len(&buf[hl..]) {
                Some(n) => n,
                None => return Ok(None),
            },
            Encoding::Zrle => {
                let Some(len) = buf.get(hl..hl + 4) else { return Ok(None) };
                let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as u64;
                // Compressed data can't sensibly exceed raw size plus slack.
                let cap = r.area() * 5 + 64 * 1024;
                if len > cap {
                    return Err(proto(format!("ZRLE payload of {len} bytes for {r:?} is implausibly large")));
                }
                4 + len as usize
            }
            Encoding::Tight => {
                if !r.fits(fw, fh) {
                    return Err(proto(format!("Tight rect {r:?} outside {fw}x{fh} framebuffer")));
                }
                match tight::payload_len(&buf[hl..], &self.pf, r.width, r.height).map_err(proto)? {
                    Some(n) => n,
                    None => return Ok(None),
                }
            }
            other => return Err(proto(format!("server used encoding {other:?}, which was not negotiated"))),
        };
        if buf.len() < hl + payload {
            return Ok(None);
        }
        let data = &buf[hl..hl + payload];
        match self.rect_counts.iter_mut().find(|(e, _)| *e == h.encoding) {
            Some((_, n)) => *n += 1,
            None => self.rect_counts.push((h.encoding, 1)),
        }
        match h.encoding {
            Encoding::Raw => {
                if !r.fits(fw, fh) {
                    return Err(proto(format!("Raw rect {r:?} outside {fw}x{fh} framebuffer")));
                }
                self.fb.put_raw(r, data, &self.pf, &self.colour_map);
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::Rre => {
                if !r.fits(fw, fh) {
                    return Err(proto(format!("RRE rect {r:?} outside {fw}x{fh} framebuffer")));
                }
                classic::decode_rre(r, data, &self.pf, &self.colour_map, &mut self.fb).map_err(proto)?;
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::Hextile => {
                classic::decode_hextile(r, data, &self.pf, &self.colour_map, &mut self.fb).map_err(proto)?;
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::CopyRect => {
                let sx = u16::from_be_bytes([data[0], data[1]]);
                let sy = u16::from_be_bytes([data[2], data[3]]);
                let src = Rect { x: sx, y: sy, ..r };
                if !r.fits(fw, fh) || !src.fits(fw, fh) {
                    return Err(proto(format!("CopyRect {r:?} from ({sx},{sy}) outside {fw}x{fh} framebuffer")));
                }
                self.fb.copy_rect(r, sx, sy);
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::Zrle => {
                if !r.fits(fw, fh) {
                    return Err(proto(format!("ZRLE rect {r:?} outside {fw}x{fh} framebuffer")));
                }
                self.zrle.decode(r, &data[4..], &self.pf, &self.colour_map, &mut self.fb).map_err(proto)?;
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::Tight => {
                self.tight.decode(r, data, &self.pf, &self.colour_map, &mut self.fb).map_err(proto)?;
                self.damage.push(r);
                self.painted = true;
            }
            Encoding::DesktopSize => {
                self.check_size(h.width, h.height)?;
                self.fb.resize(h.width, h.height);
                self.painted = false;
                self.refresh_after_resize = true;
                self.damage.clear();
                self.damage.push(Rect { x: 0, y: 0, width: h.width, height: h.height });
                events.push(Event::Resized { width: h.width, height: h.height });
            }
            Encoding::ExtendedDesktopSize => {
                let screens = desktop::parse(data).map_err(proto)?;
                let (reason, status) = (h.x, h.y);
                if reason == desktop::REASON_THIS_CLIENT && status != desktop::STATUS_OK {
                    events.push(Event::ResizeRefused { status, reason: desktop::status_text(status) });
                } else {
                    rfb_proto::validate_layout(h.width, h.height, &screens)
                        .map_err(|e| proto(format!("ExtendedDesktopSize: {e}")))?;
                    self.check_size(h.width, h.height)?;
                    self.screens = Some(screens);
                    if (h.width, h.height) != (fw, fh) {
                        self.fb.resize(h.width, h.height);
                        self.painted = false;
                        self.refresh_after_resize = true;
                        self.damage.clear();
                        self.damage.push(Rect { x: 0, y: 0, width: h.width, height: h.height });
                        events.push(Event::Resized { width: h.width, height: h.height });
                    }
                }
            }
            Encoding::Cursor => {
                let shape =
                    cursor::decode(data, &self.pf, &self.colour_map, h.x, h.y, h.width, h.height).map_err(proto)?;
                self.cursor = Some(shape.clone());
                events.push(Event::CursorShape(shape));
            }
            _ => unreachable!("filtered above"),
        }
        self.rects_left -= 1;
        if self.rects_left == 0 {
            self.finish_update(events);
        }
        Ok(Some(hl + payload))
    }

    fn finish_update(&mut self, events: &mut Vec<Event>) {
        events.push(Event::Updated { damage: std::mem::take(&mut self.damage) });
        if std::mem::take(&mut self.refresh_after_resize) && !self.painted {
            // The size changed but no pixels came with it, so the screen is
            // black; ask for everything once, even without continuous updates.
            self.request_update(false);
        } else if self.opts.continuous_updates {
            self.request_update(true);
        }
    }

    /// Queue a FramebufferUpdateRequest for the whole screen.
    pub fn request_update(&mut self, incremental: bool) {
        if !self.is_connected() {
            return;
        }
        ClientMessage::FramebufferUpdateRequest {
            incremental,
            x: 0,
            y: 0,
            width: self.fb.width(),
            height: self.fb.height(),
        }
        .encode(&mut self.out);
    }

    /// Key press or release, as an X11 keysym (RFC 6143 §7.5.4).
    pub fn key(&mut self, keysym: u32, down: bool) {
        if self.is_connected() {
            ClientMessage::KeyEvent { down, keysym }.encode(&mut self.out);
        }
    }

    /// Pointer position and full button mask (bit 0 = left, 1 = middle, 2 = right,
    /// 3/4 = wheel up/down). Coordinates are clamped to the framebuffer.
    pub fn pointer(&mut self, x: u16, y: u16, button_mask: u8) {
        if self.is_connected() {
            self.buttons = button_mask;
            let x = x.min(self.fb.width().saturating_sub(1));
            let y = y.min(self.fb.height().saturating_sub(1));
            ClientMessage::PointerEvent { button_mask, x, y }.encode(&mut self.out);
        }
    }

    /// One wheel notch: press and release button 4 (up) or 5 (down).
    pub fn scroll(&mut self, x: u16, y: u16, up: bool) {
        let bit = if up { 1 << 3 } else { 1 << 4 };
        let held = self.buttons;
        self.pointer(x, y, held | bit);
        self.pointer(x, y, held);
    }

    /// Our clipboard now holds `text`. With Extended Clipboard the server is
    /// told and fetches the UTF-8 text when it needs it; otherwise it is sent
    /// right away as Latin-1. Text set before the handshake finishes is
    /// announced once it does. Repeating the current text sends nothing.
    pub fn set_clipboard(&mut self, text: &str) {
        let connected = self.is_connected();
        for m in self.clipboard.set_local(text, connected) {
            match m {
                crate::clipboard::Outgoing::Extended(m) => ClientMessage::ExtendedClipboard(m).encode(&mut self.out),
                crate::clipboard::Outgoing::Legacy(b) => ClientMessage::ClientCutText(b).encode(&mut self.out),
            }
        }
    }

    /// The server negotiated Extended Clipboard (UTF-8 text, lazy transfer).
    pub fn clipboard_extended(&self) -> bool {
        self.clipboard.is_extended()
    }

    /// Send raw RFC 6143 cut text (Latin-1), bypassing Extended Clipboard.
    /// Prefer [`Session::set_clipboard`].
    pub fn cut_text(&mut self, text: &[u8]) {
        if self.is_connected() {
            ClientMessage::ClientCutText(text.to_vec()).encode(&mut self.out);
        }
    }
}
