//! Aurora Video: compressed video (AV1, optionally HEVC or H.264) carried in
//! FramebufferUpdate rectangles, split into independent tiles so several
//! hardware encoders and decoders can work on one frame in parallel.
//! The normative description is `spec/VIDEO.md`; this module is its codec.
//!
//! Two pieces go on the wire:
//!
//! - **Aurora messages** (message type [`AURORA_MSG_TYPE`] = 65 in both
//!   directions): an 8-byte envelope `type:u8, subtype:u8, reserved:u16,
//!   length:u32` and a body of `length` bytes. Unknown subtypes are kept as
//!   `Unknown` and skipped, and bytes after the fields a receiver knows are
//!   ignored, so later versions can append fields.
//! - **Video rectangles** (encoding [`VIDEO_ENCODING`] = 0x41555200): a
//!   32-byte [`VideoRectHeader`] and `length` bytes of codec data, one
//!   temporal unit / access unit for one tile.
//!
//! Negotiation: the client lists [`VIDEO_ENCODING`] in SetEncodings; a
//! server that speaks it sends [`AuroraServerMessage::VideoOffer`]; the
//! client answers with [`AuroraClientMessage::VideoCaps`]; the server
//! picks a codec and tiling and sends [`AuroraServerMessage::VideoConfig`]
//! (codec [`Codec::NONE`] means classic encodings only).

use std::fmt;

use crate::codec::{len_u32, DecodeError, Limits, PutBe, Reader};

/// Encoding number of an Aurora Video rectangle (ASCII "AUR\0"), from the
/// Aurora block in `spec/EXTENSIONS.md`.
pub const VIDEO_ENCODING: i32 = 0x4155_5200;
/// Message type of every Aurora message, client-to-server and
/// server-to-client (ASCII 'A').
pub const AURORA_MSG_TYPE: u8 = 65;
/// Version of the video protocol described here.
pub const VIDEO_VERSION: u16 = 1;
/// Bytes before an Aurora message body.
pub const ENVELOPE_LEN: usize = 8;
/// `tile` value in a KeyframeRequest meaning every tile.
pub const ALL_TILES: u16 = 0xffff;
/// "Not measured" in FrameAck timing fields.
pub const UNKNOWN_US: u32 = u32::MAX;
/// "No stated limit" for a level.
pub const ANY_LEVEL: u8 = 0xff;

/// Codec identifiers (one byte). Unknown values are carried as-is; a
/// server never picks a codec it does not know.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Codec(pub u8);

impl Codec {
    /// No video: classic encodings only (VideoConfig only).
    pub const NONE: Codec = Codec(0);
    /// AV1, Low Overhead Bitstream Format; mandatory to implement.
    pub const AV1: Codec = Codec(1);
    /// HEVC / H.265, Annex B byte stream; optional.
    pub const HEVC: Codec = Codec(2);
    /// H.264 / AVC, Annex B byte stream; optional.
    pub const H264: Codec = Codec(3);
    /// First and last ids reserved for learned codecs ("tiers"), each bound
    /// to one model by the SHA-256 of its weights (spec/LEARNED.md).
    pub const LEARNED_FIRST: u8 = 128;
    pub const LEARNED_LAST: u8 = 191;

    /// A learned-codec id (128 ..= 191).
    pub fn is_learned(self) -> bool {
        (Self::LEARNED_FIRST..=Self::LEARNED_LAST).contains(&self.0)
    }

    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Codec::NONE => "None",
            Codec::AV1 => "AV1",
            Codec::HEVC => "HEVC",
            Codec::H264 => "H264",
            _ => return None,
        })
    }

    /// Bit for this codec in [`AuroraServerMessage::VideoOffer`] `codecs`.
    pub fn mask(self) -> u32 {
        if self.0 < 32 {
            1 << self.0
        } else {
            0
        }
    }
}

impl fmt::Debug for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => f.write_str(n),
            None if self.is_learned() => write!(f, "Learned({})", self.0),
            None => write!(f, "Codec({})", self.0),
        }
    }
}

/// Codec capability / stream flags.
pub mod flags {
    /// 10-bit samples.
    pub const DEPTH_10: u8 = 1 << 0;
    /// 4:4:4 chroma (otherwise 4:2:0).
    pub const CHROMA_444: u8 = 1 << 1;
    /// Caps only: this entry is decoded in hardware (informational).
    pub const HARDWARE: u8 = 1 << 2;
}

/// Video rectangle flags.
pub mod rect_flags {
    /// The data is a random access point (AV1 key frame with a sequence
    /// header; HEVC IRAP or H.264 IDR with parameter sets).
    pub const KEYFRAME: u16 = 1 << 0;
    /// Drop this tile's decoder state before decoding. With `length` 0 the
    /// rectangle only resets (flushes) the tile.
    pub const RESET: u16 = 1 << 1;
}

/// KeyframeRequest reasons.
pub mod reason {
    pub const UNSPECIFIED: u8 = 0;
    /// The client could not decode the tile.
    pub const DECODE_ERROR: u8 = 1;
    /// The client dropped data (for example a slow decoder skipped frames).
    pub const LOSS: u8 = 2;
    /// Start over: the server sends a fresh VideoConfig (new id) and a key
    /// frame in every tile.
    pub const RESET: u8 = 3;
}

/// One codec the client can decode, in its order of preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecCap {
    pub codec: Codec,
    /// Highest profile: AV1 `seq_profile`, HEVC `general_profile_idc`,
    /// H.264 `profile_idc`.
    pub profile: u8,
    /// Highest level (AV1 `seq_level_idx`, HEVC `general_level_idc`, H.264
    /// `level_idc`), or [`ANY_LEVEL`].
    pub level: u8,
    /// [`flags`] bits.
    pub flags: u8,
    /// Largest tile one decoder instance takes, pixels.
    pub max_width: u16,
    pub max_height: u16,
    /// Highest frame rate per decoder instance, frames per second.
    pub max_fps: u16,
    /// Decoder instances (tiles) the client can run at once with this codec.
    pub max_tiles: u16,
}

impl CodecCap {
    pub const WIRE_LEN: usize = 12;
}

/// Client capabilities (Aurora client message subtype 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCaps {
    /// Highest version the client speaks.
    pub version: u16,
    /// Reserved, 0.
    pub flags: u16,
    /// Frames the server may send before the client acknowledges them
    /// (FrameAck). 0: send video only in reply to FramebufferUpdateRequest.
    pub max_frames_in_flight: u8,
    /// Highest bit rate the client wants, kbit/s; 0 = no limit.
    pub max_bitrate_kbps: u32,
    /// Total decode throughput, thousands of pixels per second; 0 = unknown.
    pub max_pixel_rate_kpps: u32,
    /// Supported codecs, most preferred first; empty turns video off.
    pub codecs: Vec<CodecCap>,
}

/// Measured decode throughput for one tier: a codec with a tile format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierPerf {
    pub codec: Codec,
    /// [`tile_format`] bits this measurement is for (chroma, depth, RGB,
    /// lossless).
    pub format: u8,
    /// Decoder threads (or GPU decode sessions) the client runs for it.
    pub threads: u16,
    /// Pixels per second one tile decodes at, in thousands: the speed of a
    /// single decoder instance (a learned tier that decodes serially within
    /// a tile is bounded by this).
    pub tile_kpps: u32,
    /// Pixels per second over all `threads` together, in thousands.
    pub total_kpps: u32,
}

impl TierPerf {
    pub const WIRE_LEN: usize = 12;
}

/// Client decode performance (Aurora client message subtype 5): sent after
/// VideoCaps and again whenever the measurements change. The server keeps
/// every frame it sends decodable within the frame deadline
/// ([`DecodePerf::fits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodePerf {
    /// Tiles the client can hold undecoded at once (across unacknowledged
    /// frames); 0 = no stated limit.
    pub max_tiles_in_flight: u16,
    /// Display refresh rate in millihertz (239760 = 239.76 Hz); 0 = unknown.
    pub refresh_mhz: u32,
    /// One entry per measured tier.
    pub tiers: Vec<TierPerf>,
}

impl DecodePerf {
    pub const FIXED_LEN: usize = 12;

    /// The measurement for a tile coded with `codec` and `format`.
    pub fn tier(&self, codec: Codec, format: u8) -> Option<&TierPerf> {
        self.tiers.iter().find(|t| t.codec == codec && t.format == format)
    }

    /// Frame deadline in microseconds at `fps`, capped by the display
    /// refresh when known.
    pub fn deadline_us(&self, fps: u16) -> u64 {
        let fps_m = u64::from(fps.max(1)) * 1000;
        let rate_m = if self.refresh_mhz == 0 { fps_m } else { fps_m.min(u64::from(self.refresh_mhz)) };
        1_000_000_000 / rate_m.max(1)
    }

    /// Whether a frame of `tiles` (codec, format, pixels) decodes within
    /// the deadline at `fps` (spec/VIDEO.md section 6.6): every tile within
    /// its tier's single-instance speed, and the sum over tiers of each
    /// tier's pixels at its total speed within the deadline. A tier with
    /// no measurement fits only if it isn't learned (standard tiers fall
    /// back to VideoCaps limits); learned tiers need a measurement.
    pub fn fits(&self, tiles: &[(Codec, u8, u32)], fps: u16) -> bool {
        let deadline = self.deadline_us(fps) as f64;
        let mut busy_us = 0.0f64;
        let mut seen: Vec<(Codec, u8, u64)> = Vec::new();
        for &(codec, format, px) in tiles {
            match self.tier(codec, format) {
                Some(t) => {
                    if t.tile_kpps == 0 || f64::from(px) * 1000.0 / f64::from(t.tile_kpps) > deadline {
                        return false;
                    }
                    match seen.iter_mut().find(|(c, f, _)| *c == codec && *f == format) {
                        Some(e) => e.2 += u64::from(px),
                        None => seen.push((codec, format, u64::from(px))),
                    }
                }
                None if codec.is_learned() => return false,
                None => {}
            }
        }
        for (codec, format, px) in seen {
            let t = self.tier(codec, format).expect("seen");
            busy_us += px as f64 * 1000.0 / f64::from(t.total_kpps.max(t.tile_kpps));
        }
        busy_us <= deadline
    }
}

/// The stream the server chose (Aurora server message subtype 2). A new
/// config (new `config_id`) resets every tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoConfig {
    /// Non-zero, different from the previous config on this connection.
    pub config_id: u32,
    /// [`Codec::NONE`]: video is off, classic encodings resume.
    pub codec: Codec,
    pub profile: u8,
    pub level: u8,
    /// [`flags`] bits (DEPTH_10, CHROMA_444).
    pub flags: u8,
    /// ITU-T H.273 colour description, as also signalled in the bitstream.
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub full_range: bool,
    /// Target frame rate, frames per second.
    pub fps: u16,
    /// Frames the server will send ahead of FrameAck (at most the client's
    /// `max_frames_in_flight`); 0 = request-driven.
    pub frames_in_flight: u8,
    /// Framebuffer size this config covers.
    pub width: u16,
    pub height: u16,
    /// Tile grid: column widths (left to right) and row heights (top to
    /// bottom), summing to `width` and `height`. Tile `i` is column
    /// `i % cols`, row `i / cols`.
    pub col_widths: Vec<u16>,
    pub row_heights: Vec<u16>,
}

impl VideoConfig {
    pub const FIXED_LEN: usize = 24;

    /// A config that turns video off.
    pub fn off(config_id: u32) -> Self {
        VideoConfig {
            config_id,
            codec: Codec::NONE,
            profile: 0,
            level: 0,
            flags: 0,
            colour_primaries: 0,
            transfer_characteristics: 0,
            matrix_coefficients: 0,
            full_range: false,
            fps: 0,
            frames_in_flight: 0,
            width: 0,
            height: 0,
            col_widths: vec![],
            row_heights: vec![],
        }
    }

    pub fn tile_count(&self) -> usize {
        self.col_widths.len() * self.row_heights.len()
    }

    /// `(x, y, width, height)` of tile `i`.
    pub fn tile_rect(&self, i: usize) -> Option<(u16, u16, u16, u16)> {
        let cols = self.col_widths.len();
        if i >= self.tile_count() {
            return None;
        }
        let (c, r) = (i % cols, i / cols);
        let x: u32 = self.col_widths[..c].iter().map(|&w| u32::from(w)).sum();
        let y: u32 = self.row_heights[..r].iter().map(|&h| u32::from(h)).sum();
        Some((x as u16, y as u16, self.col_widths[c], self.row_heights[r]))
    }

    /// The tile whose rectangle is exactly this one.
    pub fn tile_at(&self, x: u16, y: u16, w: u16, h: u16) -> Option<usize> {
        (0..self.tile_count()).find(|&i| self.tile_rect(i) == Some((x, y, w, h)))
    }

    /// Splits `len` into `n` parts, each a multiple of `align` except the
    /// last, as even as possible. None if `n` is 0 or the parts would be empty.
    pub fn split(len: u16, n: usize, align: u16) -> Option<Vec<u16>> {
        let align = u32::from(align.max(1));
        let (len32, n32) = (u32::from(len), u32::try_from(n).ok()?);
        if n32 == 0 || n32 > len32 {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        let mut done = 0u32;
        for i in 1..=n32 {
            let end = if i == n32 { len32 } else { (len32 * i / n32).div_ceil(align) * align };
            if end <= done || end > len32 {
                return None;
            }
            out.push((end - done) as u16);
            done = end;
        }
        Some(out)
    }

    /// Checks the rules a receiver enforces.
    pub fn validate(&self, limits: &Limits) -> Result<(), DecodeError> {
        let bad = |m: &str| Err(DecodeError::Invalid(format!("VideoConfig: {m}")));
        if self.config_id == 0 {
            return bad("config_id 0");
        }
        if self.codec == Codec::NONE {
            return Ok(());
        }
        let (cols, rows) = (self.col_widths.len(), self.row_heights.len());
        if cols == 0 || rows == 0 {
            return bad("no tiles");
        }
        if cols * rows > usize::from(limits.max_video_tiles) {
            return bad("too many tiles");
        }
        if self.col_widths.contains(&0) || self.row_heights.contains(&0) {
            return bad("empty tile");
        }
        let sw: u32 = self.col_widths.iter().map(|&w| u32::from(w)).sum();
        let sh: u32 = self.row_heights.iter().map(|&h| u32::from(h)).sum();
        if sw != u32::from(self.width) || sh != u32::from(self.height) {
            return bad("tile grid does not cover the framebuffer");
        }
        if self.fps == 0 {
            return bad("fps 0");
        }
        Ok(())
    }
}

/// Aurora client-to-server messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuroraClientMessage {
    /// Subtype 1.
    VideoCaps(VideoCaps),
    /// Subtype 2: the next rectangle for `tile` ([`ALL_TILES`] for all)
    /// should be a key frame.
    KeyframeRequest { config_id: u32, tile: u16, reason: u8 },
    /// Subtype 3: every frame up to and including `frame` has been decoded
    /// and presented. Timings are the client's, in microseconds, or
    /// [`UNKNOWN_US`].
    FrameAck { config_id: u32, frame: u32, latency_us: u32, decode_us: u32 },
    /// Subtype 4: clock alignment probe, the client's clock in microseconds.
    TimeProbe { client_time_us: u64 },
    /// Subtype 5: measured decode throughput, tiles in flight, refresh rate.
    DecodePerf(DecodePerf),
    /// A subtype this version doesn't know; ignored.
    Unknown { subtype: u8, body: Vec<u8> },
}

/// Aurora server-to-client messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuroraServerMessage {
    /// Subtype 1: the server speaks Aurora Video up to `version` and can
    /// encode the codecs whose [`Codec::mask`] bits are set, in up to
    /// `max_tiles` parallel streams.
    VideoOffer { version: u16, codecs: u32, max_tiles: u16 },
    /// Subtype 2.
    VideoConfig(VideoConfig),
    /// Subtype 3: answer to TimeProbe, with the server's capture clock
    /// (the clock of `capture_time_us`) when it replied.
    TimeProbeReply { client_time_us: u64, server_time_us: u64 },
    /// A subtype this version doesn't know; ignored.
    Unknown { subtype: u8, body: Vec<u8> },
}

fn envelope(subtype: u8, body: &[u8], out: &mut Vec<u8>) {
    out.put_u8(AURORA_MSG_TYPE);
    out.put_u8(subtype);
    out.put_u16(0);
    out.put_u32(len_u32(body.len(), "Aurora message"));
    out.extend_from_slice(body);
}

/// Reads an envelope whose type byte is already consumed; returns
/// `(subtype, body)`.
pub(crate) fn read_envelope<'a>(r: &mut Reader<'a>, limits: &Limits) -> Result<(u8, &'a [u8]), DecodeError> {
    let subtype = r.u8()?;
    r.skip(2)?;
    let body = r.len_prefixed(limits.max_aurora_msg_len, "Aurora message")?;
    Ok((subtype, body))
}

fn short(what: &str) -> impl Fn(DecodeError) -> DecodeError + '_ {
    move |_| DecodeError::Invalid(format!("{what} body too short"))
}

impl AuroraClientMessage {
    pub fn subtype(&self) -> u8 {
        match self {
            AuroraClientMessage::VideoCaps(_) => 1,
            AuroraClientMessage::KeyframeRequest { .. } => 2,
            AuroraClientMessage::FrameAck { .. } => 3,
            AuroraClientMessage::TimeProbe { .. } => 4,
            AuroraClientMessage::DecodePerf(_) => 5,
            AuroraClientMessage::Unknown { subtype, .. } => *subtype,
        }
    }

    /// The whole message, envelope included.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut b = Vec::new();
        match self {
            AuroraClientMessage::VideoCaps(c) => {
                b.put_u16(c.version);
                b.put_u16(c.flags);
                b.put_u8(c.max_frames_in_flight);
                b.put_u8(u8::try_from(c.codecs.len()).expect("more than 255 codec entries"));
                b.put_u16(0);
                b.put_u32(c.max_bitrate_kbps);
                b.put_u32(c.max_pixel_rate_kpps);
                for e in &c.codecs {
                    b.extend_from_slice(&[e.codec.0, e.profile, e.level, e.flags]);
                    b.put_u16(e.max_width);
                    b.put_u16(e.max_height);
                    b.put_u16(e.max_fps);
                    b.put_u16(e.max_tiles);
                }
            }
            AuroraClientMessage::KeyframeRequest { config_id, tile, reason } => {
                b.put_u32(*config_id);
                b.put_u16(*tile);
                b.put_u8(*reason);
                b.put_u8(0);
            }
            AuroraClientMessage::FrameAck { config_id, frame, latency_us, decode_us } => {
                b.put_u32(*config_id);
                b.put_u32(*frame);
                b.put_u32(*latency_us);
                b.put_u32(*decode_us);
            }
            AuroraClientMessage::TimeProbe { client_time_us } => b.put_u64(*client_time_us),
            AuroraClientMessage::DecodePerf(p) => {
                b.put_u16(p.max_tiles_in_flight);
                b.put_u16(0);
                b.put_u32(p.refresh_mhz);
                b.put_u8(u8::try_from(p.tiers.len()).expect("more than 255 tiers"));
                b.extend_from_slice(&[0, 0, 0]);
                for t in &p.tiers {
                    b.extend_from_slice(&[t.codec.0, t.format]);
                    b.put_u16(t.threads);
                    b.put_u32(t.tile_kpps);
                    b.put_u32(t.total_kpps);
                }
            }
            AuroraClientMessage::Unknown { body, .. } => b.extend_from_slice(body),
        }
        envelope(self.subtype(), &b, out);
    }

    /// Decodes a body (the envelope already read).
    pub fn decode_body(subtype: u8, body: &[u8], _limits: &Limits) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        Ok(match subtype {
            1 => {
                let e = short("VideoCaps");
                let (version, flags) = (r.u16().map_err(&e)?, r.u16().map_err(&e)?);
                let max_frames_in_flight = r.u8().map_err(&e)?;
                let n = r.u8().map_err(&e)?;
                r.skip(2).map_err(&e)?;
                let max_bitrate_kbps = r.u32().map_err(&e)?;
                let max_pixel_rate_kpps = r.u32().map_err(&e)?;
                let mut codecs = Vec::with_capacity(usize::from(n));
                for _ in 0..n {
                    let b = r.bytes(CodecCap::WIRE_LEN).map_err(&e)?;
                    let u = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
                    codecs.push(CodecCap {
                        codec: Codec(b[0]),
                        profile: b[1],
                        level: b[2],
                        flags: b[3],
                        max_width: u(4),
                        max_height: u(6),
                        max_fps: u(8),
                        max_tiles: u(10),
                    });
                }
                AuroraClientMessage::VideoCaps(VideoCaps {
                    version,
                    flags,
                    max_frames_in_flight,
                    max_bitrate_kbps,
                    max_pixel_rate_kpps,
                    codecs,
                })
            }
            2 => {
                let e = short("KeyframeRequest");
                let config_id = r.u32().map_err(&e)?;
                let tile = r.u16().map_err(&e)?;
                let reason = r.u8().map_err(&e)?;
                AuroraClientMessage::KeyframeRequest { config_id, tile, reason }
            }
            3 => {
                let e = short("FrameAck");
                AuroraClientMessage::FrameAck {
                    config_id: r.u32().map_err(&e)?,
                    frame: r.u32().map_err(&e)?,
                    latency_us: r.u32().map_err(&e)?,
                    decode_us: r.u32().map_err(&e)?,
                }
            }
            4 => AuroraClientMessage::TimeProbe { client_time_us: r.u64().map_err(short("TimeProbe"))? },
            5 => {
                let e = short("DecodePerf");
                let max_tiles_in_flight = r.u16().map_err(&e)?;
                r.skip(2).map_err(&e)?;
                let refresh_mhz = r.u32().map_err(&e)?;
                let n = r.u8().map_err(&e)?;
                r.skip(3).map_err(&e)?;
                let mut tiers = Vec::with_capacity(usize::from(n));
                for _ in 0..n {
                    let b = r.bytes(TierPerf::WIRE_LEN).map_err(&e)?;
                    let u32_at = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
                    tiers.push(TierPerf {
                        codec: Codec(b[0]),
                        format: b[1],
                        threads: u16::from_be_bytes([b[2], b[3]]),
                        tile_kpps: u32_at(4),
                        total_kpps: u32_at(8),
                    });
                }
                AuroraClientMessage::DecodePerf(DecodePerf { max_tiles_in_flight, refresh_mhz, tiers })
            }
            subtype => AuroraClientMessage::Unknown { subtype, body: body.to_vec() },
        })
    }
}

impl AuroraServerMessage {
    pub fn subtype(&self) -> u8 {
        match self {
            AuroraServerMessage::VideoOffer { .. } => 1,
            AuroraServerMessage::VideoConfig(_) => 2,
            AuroraServerMessage::TimeProbeReply { .. } => 3,
            AuroraServerMessage::Unknown { subtype, .. } => *subtype,
        }
    }

    /// The whole message, envelope included.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut b = Vec::new();
        match self {
            AuroraServerMessage::VideoOffer { version, codecs, max_tiles } => {
                b.put_u16(*version);
                b.put_u16(0);
                b.put_u32(*codecs);
                b.put_u16(*max_tiles);
                b.put_u16(0);
            }
            AuroraServerMessage::VideoConfig(c) => {
                b.put_u32(c.config_id);
                b.extend_from_slice(&[c.codec.0, c.profile, c.level, c.flags]);
                b.extend_from_slice(&[
                    c.colour_primaries,
                    c.transfer_characteristics,
                    c.matrix_coefficients,
                    c.full_range as u8,
                ]);
                b.put_u16(c.fps);
                b.put_u8(c.frames_in_flight);
                b.put_u8(0);
                b.put_u16(c.width);
                b.put_u16(c.height);
                b.put_u8(u8::try_from(c.col_widths.len()).expect("more than 255 tile columns"));
                b.put_u8(u8::try_from(c.row_heights.len()).expect("more than 255 tile rows"));
                b.put_u16(0);
                for &w in c.col_widths.iter().chain(&c.row_heights) {
                    b.put_u16(w);
                }
            }
            AuroraServerMessage::TimeProbeReply { client_time_us, server_time_us } => {
                b.put_u64(*client_time_us);
                b.put_u64(*server_time_us);
            }
            AuroraServerMessage::Unknown { body, .. } => b.extend_from_slice(body),
        }
        envelope(self.subtype(), &b, out);
    }

    /// Decodes a body (the envelope already read). A VideoConfig is
    /// validated ([`VideoConfig::validate`]).
    pub fn decode_body(subtype: u8, body: &[u8], limits: &Limits) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        Ok(match subtype {
            1 => {
                let e = short("VideoOffer");
                let version = r.u16().map_err(&e)?;
                r.skip(2).map_err(&e)?;
                let codecs = r.u32().map_err(&e)?;
                let max_tiles = r.u16().map_err(&e)?;
                AuroraServerMessage::VideoOffer { version, codecs, max_tiles }
            }
            2 => {
                let e = short("VideoConfig");
                let b = r.bytes(VideoConfig::FIXED_LEN).map_err(&e)?;
                let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
                let (cols, rows) = (usize::from(b[20]), usize::from(b[21]));
                let grid = r.bytes(2 * (cols + rows)).map_err(&e)?;
                let mut g = grid.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                let c = VideoConfig {
                    config_id: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                    codec: Codec(b[4]),
                    profile: b[5],
                    level: b[6],
                    flags: b[7],
                    colour_primaries: b[8],
                    transfer_characteristics: b[9],
                    matrix_coefficients: b[10],
                    full_range: b[11] != 0,
                    fps: u16_at(12),
                    frames_in_flight: b[14],
                    width: u16_at(16),
                    height: u16_at(18),
                    col_widths: g.by_ref().take(cols).collect(),
                    row_heights: g.collect(),
                };
                c.validate(limits)?;
                if c.codec == Codec::NONE {
                    AuroraServerMessage::VideoConfig(VideoConfig::off(c.config_id))
                } else {
                    AuroraServerMessage::VideoConfig(c)
                }
            }
            3 => {
                let e = short("TimeProbeReply");
                AuroraServerMessage::TimeProbeReply {
                    client_time_us: r.u64().map_err(&e)?,
                    server_time_us: r.u64().map_err(&e)?,
                }
            }
            subtype => AuroraServerMessage::Unknown { subtype, body: body.to_vec() },
        })
    }
}

/// Per-tile picture format (the `format` byte of a video rectangle).
pub mod tile_format {
    /// Bits 0-1: chroma subsampling.
    pub const CHROMA_MASK: u8 = 0b11;
    /// 4:2:0, chroma sited "left" (see spec/VIDEO.md section 5.2).
    pub const CHROMA_420: u8 = 0;
    /// 4:4:4.
    pub const CHROMA_444: u8 = 1;
    /// 10-bit samples (otherwise 8-bit).
    pub const DEPTH_10: u8 = 1 << 2;
    /// Planes are G, B, R with the identity matrix (H.273 matrix 0); only
    /// with 4:4:4. Otherwise Y'CbCr with the config's matrix and range.
    pub const RGB: u8 = 1 << 3;
    /// The tile was coded losslessly: decoded samples equal the source.
    pub const LOSSLESS: u8 = 1 << 4;
    /// Bits a version 1 sender may set.
    pub const KNOWN: u8 = 0b1_1111;
}

/// The 32 bytes at the start of an Aurora Video rectangle's payload. The
/// header is self-contained: a client can split a frame into tiles and hand
/// each to a worker using only these fields, before decoding any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoRectHeader {
    /// The VideoConfig this belongs to; it must be the active one.
    pub config_id: u32,
    /// Frame number: the same for every tile of one captured frame, and
    /// increasing by one (wrapping) from frame to frame.
    pub frame: u32,
    /// When the frame was captured, server clock, microseconds.
    pub capture_time_us: u64,
    /// Tile index in the config's grid (its position); the rectangle must be
    /// the tile's.
    pub tile: u16,
    /// This tile's sequence number within the frame, 0 ..= `tiles_in_frame - 1`.
    pub seq: u16,
    /// How many tiles this frame carries (the frame is complete when all
    /// have arrived). At least 1.
    pub tiles_in_frame: u16,
    /// [`rect_flags`] bits.
    pub flags: u16,
    /// Codec of this tile's data (one the client listed in VideoCaps).
    pub codec: Codec,
    /// [`tile_format`] bits.
    pub format: u8,
    /// Bytes of codec data that follow.
    pub length: u32,
}

impl VideoRectHeader {
    pub const WIRE_LEN: usize = 32;

    pub fn is_keyframe(&self) -> bool {
        self.flags & rect_flags::KEYFRAME != 0
    }

    pub fn is_reset(&self) -> bool {
        self.flags & rect_flags::RESET != 0
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u32(self.config_id);
        out.put_u32(self.frame);
        out.put_u64(self.capture_time_us);
        out.put_u16(self.tile);
        out.put_u16(self.seq);
        out.put_u16(self.tiles_in_frame);
        out.put_u16(self.flags);
        out.put_u8(self.codec.0);
        out.put_u8(self.format);
        out.put_u16(0);
        out.put_u32(self.length);
    }

    /// Decodes the header and borrows the codec data that follows. The whole
    /// payload must be present (`Incomplete` otherwise, sized to the end of
    /// the data). The structural rules are checked first: `length` within
    /// `max_video_payload`, `seq < tiles_in_frame`, `tiles_in_frame` within
    /// `max_video_tiles`, RGB only with 4:4:4, and a known chroma value.
    pub fn decode<'a>(buf: &'a [u8], limits: &Limits) -> Result<(Self, &'a [u8], usize), DecodeError> {
        let mut r = Reader::new(buf);
        let config_id = r.u32()?;
        let frame = r.u32()?;
        let capture_time_us = r.u64()?;
        let tile = r.u16()?;
        let seq = r.u16()?;
        let tiles_in_frame = r.u16()?;
        let flags = r.u16()?;
        let codec = Codec(r.u8()?);
        let format = r.u8()?;
        r.skip(2)?;
        let length = r.u32()?;
        let bad = |m: String| Err(DecodeError::Invalid(format!("video rectangle: {m}")));
        if length > limits.max_video_payload {
            return bad(format!("payload {length} exceeds limit {}", limits.max_video_payload));
        }
        if tiles_in_frame == 0 || tiles_in_frame > limits.max_video_tiles || seq >= tiles_in_frame {
            return bad(format!("tile {seq} of {tiles_in_frame}"));
        }
        if tile >= limits.max_video_tiles {
            return bad(format!("tile index {tile}"));
        }
        let chroma = format & tile_format::CHROMA_MASK;
        if chroma > tile_format::CHROMA_444 || (format & tile_format::RGB != 0 && chroma != tile_format::CHROMA_444) {
            return bad(format!("format {format:#04x}"));
        }
        let data = r.bytes(length as usize)?;
        let h = VideoRectHeader {
            config_id,
            frame,
            capture_time_us,
            tile,
            seq,
            tiles_in_frame,
            flags,
            codec,
            format,
            length,
        };
        Ok((h, data, r.consumed()))
    }
}

/// Writes a whole Aurora Video rectangle: the RFB rectangle header for
/// tile `h.tile` of `config`, the video header and `data`.
pub fn encode_rect(config: &VideoConfig, h: &VideoRectHeader, data: &[u8], out: &mut Vec<u8>) {
    let (x, y, w, hh) = config.tile_rect(usize::from(h.tile)).expect("tile outside the config");
    crate::messages::RectHeader { x, y, width: w, height: hh, encoding: crate::types::Encoding::AuroraVideo }
        .encode(out);
    VideoRectHeader { length: len_u32(data.len(), "video payload"), ..*h }.encode(out);
    out.extend_from_slice(data);
}

/// Clock alignment from one TimeProbe round trip, NTP style: returns
/// `(offset, rtt)` in microseconds, where `server_clock ≈ client_clock +
/// offset`. `t0` is the probe's time, `t1` when the reply arrived (both
/// client clock), `s` the server time in the reply.
pub fn clock_offset(t0: u64, s: u64, t1: u64) -> (i64, u64) {
    let rtt = t1.saturating_sub(t0);
    let mid = t0 as i128 + i128::from(rtt / 2);
    ((s as i128 - mid) as i64, rtt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_aligns_and_covers() {
        assert_eq!(VideoConfig::split(7680, 4, 64).unwrap(), vec![1920, 1920, 1920, 1920]);
        assert_eq!(VideoConfig::split(4320, 2, 64).unwrap(), vec![2176, 2144]);
        assert_eq!(VideoConfig::split(100, 3, 1).unwrap().iter().sum::<u16>(), 100);
        assert!(VideoConfig::split(3, 4, 1).is_none());
        assert!(VideoConfig::split(10, 0, 1).is_none());
        assert!(VideoConfig::split(100, 3, 64).is_none());
    }

    #[test]
    fn clock_offset_symmetric() {
        assert_eq!(clock_offset(1000, 5500, 2000), (4000, 1000));
    }

    #[test]
    fn decode_perf_deadline() {
        let lossless = tile_format::CHROMA_444 | tile_format::RGB | tile_format::LOSSLESS;
        let p = DecodePerf {
            max_tiles_in_flight: 16,
            refresh_mhz: 144_000,
            tiers: vec![
                TierPerf { codec: Codec::AV1, format: 0, threads: 4, tile_kpps: 2_000_000, total_kpps: 8_000_000 },
                TierPerf { codec: Codec(130), format: lossless, threads: 2, tile_kpps: 100_000, total_kpps: 200_000 },
            ],
        };
        // 240 fps capped to the 144 Hz display.
        assert_eq!(p.deadline_us(240), 6944);
        let tile = 1920 * 1080;
        assert!(p.fits(&[(Codec::AV1, 0, tile); 4], 240));
        // 4 x 2.07 Mpx at 8 Gpx/s = 1.04 ms; 30 tiles = 7.8 ms is too slow.
        assert!(!p.fits(&[(Codec::AV1, 0, tile); 30], 240));
        // The learned tier: one 1080p tile needs 20.7 ms serially.
        assert!(!p.fits(&[(Codec(130), lossless, tile)], 240));
        assert!(p.fits(&[(Codec(130), lossless, 256 * 256)], 240));
        // Unmeasured learned tiers never fit; unmeasured standard ones do.
        assert!(!p.fits(&[(Codec(131), 0, 64)], 240));
        assert!(p.fits(&[(Codec::HEVC, 0, 64)], 240));
    }
}
