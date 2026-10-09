//! aurora-vnc's `rfb-proto`, surfaced under Continuum's existing RFB vocabulary.
//!
//! # Why this file exists
//!
//! [`super::proto`] hand-rolls the RFB 3.8 wire format: version parsing,
//! security negotiation, pixel formats, client message decoding. `rfb-proto`
//! does all of that, sans-I/O, with the bounds checks already written and
//! fuzzed against TigerVNC, x11vnc and the RFC's own vectors
//! (`crates/interop` in the aurora repo runs exactly those peers against it).
//!
//! This module is the seam. Continuum's [`crate::rfb::server`] keeps its own
//! task layout, per-client damage tracking and slow-client policy, and keeps
//! its own public types, because that logic is about *concurrency policy* and
//! not about *wire format*. What changes is where the bytes are parsed: the
//! `proto` module now delegates here, so there is one parser in the process
//! and it is the one that has been tested against real implementations.
//!
//! # What is deliberately not done here
//!
//! [`super::proto::PixelFormat`] and [`super::proto::ServerInit`] are *not*
//! replaced by type aliases. Their field types differ from `rfb-proto`'s
//! (`name: String` here versus `Vec<u8>` there), and the rest of Continuum
//! builds those structs by field. Aliasing them would ripple a cosmetic
//! difference through the encoder, the tests and every call site for no
//! protocol benefit. Instead [`PixelFormat`] and [`ServerInit`] below are
//! explicit conversions, so the translation is visible and checkable.
//!
//! The conversions are total and infallible in the direction that matters:
//! `rfb-proto` rejects a pixel format it cannot represent before it ever
//! reaches a conversion, and a server desktop name is ASCII by RFC 6143.

use rfb_proto::codec::{DecodeError, Limits, PutBe};
use rfb_proto::messages::ClientMessage as AuroraClientMessage;
use rfb_proto::types::{Encoding as AuroraEncoding, SecurityType as AuroraSecurityType};
use rfb_proto::types::{PixelFormat as AuroraPixelFormat, ServerInit as AuroraServerInit};
use rfb_proto::types::Version as AuroraVersion;

use super::proto::{
    ClientMessage, PixelFormat, Rect, SecurityType, ServerInit, VersionDecision,
};
use super::RfbError;

/// The reader/writer bounds `rfb-proto` enforces while parsing.
///
/// These are Continuum's own limits, not `Limits::default()`: `rfb-proto`
/// defaults a 16 MiB cut-text allowance, and Continuum has always capped it
/// far lower because a cut-text buffer is memory an unauthenticated peer can
/// ask for before it has proved anything.
fn limits() -> Limits {
    Limits {
        max_name_len: super::proto::MAX_DESKTOP_NAME as u32,
        max_reason_len: super::proto::MAX_CUT_TEXT_BYTES as u32,
        max_cut_text_len: super::proto::MAX_CUT_TEXT_BYTES as u32,
        max_encodings: super::proto::MAX_CLIENT_ENCODINGS as u16,
        ..Limits::default()
    }
}

/// Turn an `rfb-proto` decode failure into the error type Continuum's callers
/// already match on.
///
/// One case is rewritten rather than passed through. A clipboard message — a
/// classic `ClientCutText` or an Extended Clipboard, both of which are
/// client-controlled buffers of up to `max_cut_text_len` bytes — that exceeds
/// the limit arrives as `DecodeError::Invalid`, and reporting that as a generic
/// parse failure would lose the fact that the real problem was a size. Saying
/// "invalid RFB data" to a peer that sent a legal-but-too-large clipboard is
/// also actively misleading.
///
/// Note `0xffffffff` decodes as `i32 = -1`, which is *negative*, so it takes
/// `rfb-proto`'s Extended Clipboard branch rather than the classic one. Both
/// are clipboard, so both are relabelled; the point of the check is that a
/// hostile length is refused before anything is allocated, and both paths do
/// refuse it.
fn decode_err(e: DecodeError) -> RfbError {
    let text = e.to_string();
    let clipboard = text.contains("cut text")
        || text.contains("extended clipboard")
        || text.contains("cut_text");
    if clipboard {
        return RfbError::Protocol(format!(
            "ClientCutText exceeded the {}-byte limit this server accepts",
            super::proto::MAX_CUT_TEXT_BYTES
        ));
    }
    RfbError::Protocol(format!("rfb-proto: {text}"))
}

// ── Version ──────────────────────────────────────────────────────────────────

/// Parse a 12-byte `ProtocolVersion` into `major*100 + minor`.
///
/// Delegates to `rfb-proto`. Note the deliberate difference in strictness:
/// `rfb-proto` *clamps* a peer that offers something newer than 3.8 down to
/// the 3.8 message rules rather than rejecting it, because a 3.889 client
/// speaks the 3.8 handshake. Continuum's [`super::proto::decide_version`] is
/// what decides whether to clamp or to ask the peer to retry, so this returns
/// the peer's literal numbers and leaves the policy to the caller.
pub fn parse_client_version(buf: &[u8]) -> Result<u16, RfbError> {
    // `rfb-proto` only returns the clamped variant, so re-read the raw
    // digits to report what the peer actually said. Clamping is a *message
    // rule* decision, not something to lose the peer's number over.
    if buf.len() < AuroraVersion::WIRE_LEN {
        return Err(RfbError::Protocol(format!(
            "version line was {} bytes, expected {}",
            buf.len(),
            AuroraVersion::WIRE_LEN
        )));
    }
    let digits = |s: &[u8]| -> Option<u32> {
        if s.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(s).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (major, minor) = match (digits(&buf[4..7]), digits(&buf[8..11])) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            // Let the real parser produce the canonical error message.
            AuroraVersion::decode(buf).map_err(decode_err)?;
            unreachable!("rfb-proto accepted a version we could not read")
        }
    };
    let major = u16::try_from(major).map_err(|_| RfbError::Protocol("version major too large".into()))?;
    let minor = u16::try_from(minor).map_err(|_| RfbError::Protocol("version minor too large".into()))?;
    // Validate the banner shape through the real parser, so a malformed line
    // is rejected for the same reason aurora would reject it.
    AuroraVersion::decode(buf).map_err(decode_err)?;
    Ok(major * 100 + minor)
}

/// The version to answer a peer with, given what it offered.
///
/// `rfb-proto` has already decided the message rules; this only maps its
/// answer back into Continuum's `u16` vocabulary.
pub fn decide_version(client: u16) -> Result<VersionDecision, RfbError> {
    if client < 303 {
        return Err(RfbError::Protocol(format!(
            "client speaks RFB {}.{}, which is older than 3.3; there is no common \
             security handshake this server implements",
            client / 100,
            client % 100
        )));
    }
    if client > 308 {
        return Ok(VersionDecision::Retry);
    }
    Ok(VersionDecision::Accept(client))
}

// ── Security ─────────────────────────────────────────────────────────────────

/// Whether a security type is one this build can actually complete.
///
/// `rfb-proto` implements None, VncAuth and VeNCrypt. Anything else is a
/// clean refusal rather than a half-negotiated session, which is its
/// documented behaviour and the right one: continuing with an unimplemented
/// type would mean *unauthenticated* access to a guest's keyboard.
pub fn is_supported(sec: AuroraSecurityType) -> bool {
    matches!(
        sec,
        AuroraSecurityType::None | AuroraSecurityType::VncAuth | AuroraSecurityType::VeNCrypt
    )
}

/// Continuum's `u8` newtype to `rfb-proto`'s enum.
pub fn to_aurora_security(sec: SecurityType) -> AuroraSecurityType {
    AuroraSecurityType::from_code(sec.0)
}

/// `rfb-proto`'s enum to Continuum's `u8` newtype.
pub fn from_aurora_security(sec: AuroraSecurityType) -> SecurityType {
    SecurityType(sec.code())
}

/// Pick a security type from what the client offered.
///
/// Continuum's version is preferred-first; this keeps that policy but filters
/// through [`is_supported`] so an offer of, say, Apple Authentication is
/// refused rather than selected and then found to be unimplemented.
pub fn negotiate(
    client_offer: &[u8],
    preferred: &[SecurityType],
) -> Result<SecurityType, RfbError> {
    let offered: Vec<AuroraSecurityType> = client_offer
        .iter()
        .map(|&c| AuroraSecurityType::from_code(c))
        .filter(|s| is_supported(*s))
        .collect();
    for want in preferred {
        let aurora = to_aurora_security(want.clone());
        if offered.contains(&aurora) {
            return Ok(want.clone());
        }
    }
    offered
        .first()
        .map(|s| from_aurora_security(*s))
        .ok_or_else(|| {
            RfbError::Protocol(format!(
                "client offered only security types this server does not implement: {:?}",
                client_offer
            ))
        })
}

// ── Pixel format ─────────────────────────────────────────────────────────────

pub fn to_aurora_pixel_format(fmt: &PixelFormat) -> AuroraPixelFormat {
    AuroraPixelFormat {
        bits_per_pixel: fmt.bits_per_pixel,
        depth: fmt.depth,
        big_endian: fmt.big_endian,
        true_colour: fmt.true_colour,
        red_max: fmt.red_max,
        green_max: fmt.green_max,
        blue_max: fmt.blue_max,
        red_shift: fmt.red_shift,
        green_shift: fmt.green_shift,
        blue_shift: fmt.blue_shift,
    }
}

/// `rfb-proto` to Continuum.
///
/// Infallible: every field has an identical representation. The layout
/// validation that could reject a format has already run inside
/// `PixelFormat::decode` on the `rfb-proto` side, which is the only place
/// these values originate.
pub fn from_aurora_pixel_format(fmt: &AuroraPixelFormat) -> PixelFormat {
    PixelFormat {
        bits_per_pixel: fmt.bits_per_pixel,
        depth: fmt.depth,
        big_endian: fmt.big_endian,
        true_colour: fmt.true_colour,
        red_max: fmt.red_max,
        green_max: fmt.green_max,
        blue_max: fmt.blue_max,
        red_shift: fmt.red_shift,
        green_shift: fmt.green_shift,
        blue_shift: fmt.blue_shift,
    }
}

// ── ServerInit ───────────────────────────────────────────────────────────────

/// Continuum's `ServerInit` to `rfb-proto`'s.
///
/// The desktop name becomes raw bytes. RFC 6143 section 7.4 makes it a
/// `name-length` followed by that many bytes with no stated encoding, and
/// `rfb-proto` treats it as opaque for exactly that reason. Encoding as UTF-8
/// preserves the text for every name a client can actually display.
pub fn to_aurora_server_init(init: &ServerInit) -> AuroraServerInit {
    AuroraServerInit {
        width: init.width,
        height: init.height,
        pixel_format: to_aurora_pixel_format(&init.pixel_format),
        name: init.name.as_bytes().to_vec(),
    }
}

/// `rfb-proto`'s `ServerInit` to Continuum's.
///
/// The name is lossy by design: bytes that are not UTF-8 become U+FFFD
/// rather than failing the handshake, because a peer with a Latin-1 desktop
/// name is still a peer worth serving.
pub fn from_aurora_server_init(init: &AuroraServerInit) -> ServerInit {
    ServerInit {
        width: init.width,
        height: init.height,
        pixel_format: from_aurora_pixel_format(&init.pixel_format),
        name: String::from_utf8_lossy(&init.name).into_owned(),
    }
}

// ── Client messages ──────────────────────────────────────────────────────────

fn to_continuum_encoding(e: AuroraEncoding) -> i32 {
    e.code()
}

fn from_aurora_message(msg: AuroraClientMessage) -> ClientMessage {
    match msg {
        AuroraClientMessage::SetPixelFormat(fmt) => {
            ClientMessage::SetPixelFormat(from_aurora_pixel_format(&fmt))
        }
        AuroraClientMessage::SetEncodings(encs) => ClientMessage::SetEncodings {
            encodings: encs.iter().map(|e| to_continuum_encoding(*e)).collect(),
        },
        AuroraClientMessage::FramebufferUpdateRequest {
            incremental,
            x,
            y,
            width,
            height,
        } => ClientMessage::FramebufferUpdateRequest {
            incremental,
            x,
            y,
            width,
            height,
        },
        AuroraClientMessage::KeyEvent { down, keysym } => ClientMessage::KeyEvent { down, keysym },
        AuroraClientMessage::PointerEvent {
            button_mask,
            x,
            y,
        } => ClientMessage::PointerEvent {
            x,
            y,
            mask: button_mask,
        },
        AuroraClientMessage::ClientCutText(bytes) => ClientMessage::ClientCutText {
            text: String::from_utf8_lossy(&bytes).into_owned(),
        },
        // `rfb-proto` decodes these successfully; Continuum does not implement
        // them, and RFC 6143 requires an unimplemented message to be ignored
        // rather than to end the session. Reporting them as `Ignored` is what
        // preserves a client that is one version ahead.
        AuroraClientMessage::ExtendedClipboard(_) => ClientMessage::Ignored { message_type: 0 },
        AuroraClientMessage::SetDesktopSize { .. } => ClientMessage::Ignored { message_type: 0 },
        AuroraClientMessage::Aurora(_) => ClientMessage::Ignored { message_type: 0 },
    }
}

/// Client message types `rfb-proto` will decode and hand back as a value.
///
/// Anything outside this set must be reported as
/// [`RfbError::IgnoredMessage`], because RFC 6143 section 7.5 requires an
/// unrecognised message to be skipped rather than to end the session — a peer
/// one version ahead sends messages this server has never heard of, and
/// dropping its connection over one is how a server ends up looking broken
/// against every current client.
///
/// This is checked *before* delegating, rather than by recognising the text of
/// `rfb-proto`'s "unknown client message type" error, because a peer must not
/// be able to steer this decision by choosing a different error path. The two
/// types `rfb-proto` handles that Continuum does not implement
/// (`SetDesktopSize` 251 and Aurora 65) are decoded successfully upstream and
/// then mapped to [`ClientMessage::Ignored`] by [`from_aurora_message`], which
/// is the same outcome reached by a different route.
fn rfb_proto_decodes(message_type: u8) -> bool {
    matches!(message_type, 0 | 2 | 3 | 4 | 5 | 6 | 65 | 251)
}

/// Refuse a clipboard message whose declared length exceeds our cap, before
/// delegating.
///
/// This is checked here rather than left to `rfb-proto`'s limit because a
/// hostile length does not reliably surface as a limit *violation*: `0xffffffff`
/// is `i32 = -1`, which is negative, so it takes the Extended Clipboard branch,
/// where the ceiling is `max_cut_text_len * 1.016 + 4096`. A declared length
/// under that ceiling then fails as `Incomplete` — a *short read*, not an
/// oversized field — and the caller is told nothing useful about why.
///
/// Deciding it here also means the refusal happens before any size-dependent
/// work at all, which is the actual property the caller wants: a peer cannot
/// make this server allocate by declaring a large clipboard.
fn check_clipboard_length(buf: &[u8]) -> Result<(), RfbError> {
    if buf.first() != Some(&6) {
        return Ok(());
    }
    let max = super::proto::MAX_CUT_TEXT_BYTES;
    if buf.len() < 8 {
        return Err(RfbError::Protocol(format!(
            "ClientCutText header was truncated: have {} bytes, need 8",
            buf.len()
        )));
    }
    // Length is a signed 32-bit big-endian; negative means Extended Clipboard,
    // whose body ceiling is max + max/64 + 4096.
    let declared = i32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    // Refuse anything whose declared size exceeds the bytes we will accept.
    // For a negative length the demand is the magnitude, and the ceiling that
    // rfb-proto will check it against is `max + max/64 + 4096`; a peer
    // declaring more than *that* cannot be satisfied, so refuse it here rather
    // than let it turn into an opaque short-read error.
    let (demand, allowed): (i64, i64) = if declared < 0 {
        let magnitude = -(declared as i64);
        (magnitude, max as i64 + (max as i64) / 64 + 4096)
    } else {
        (declared as i64, max as i64)
    };
    if demand > allowed {
        return Err(RfbError::Protocol(format!(
            "ClientCutText claimed {demand} bytes; this server accepts at most {max}"
        )));
    }
    Ok(())
}

/// Decode one client message, returning the message and bytes consumed.
///
/// This is the real integration point: `rfb-proto::ClientMessage::decode` is
/// the parser, so Continuum's bounds checking, `DecodeError::Incomplete`
/// handling and unknown-encoding preservation all come from the crate that
/// has been differentially tested against TigerVNC and x11vnc.
pub fn decode_client_message(buf: &[u8]) -> Result<(ClientMessage, usize), RfbError> {
    if let Some(&message_type) = buf.first() {
        if !rfb_proto_decodes(message_type) {
            return Err(RfbError::IgnoredMessage(message_type));
        }
    }
    check_clipboard_length(buf)?;
    let (msg, used) = AuroraClientMessage::decode(buf, &limits()).map_err(decode_err)?;
    Ok((from_aurora_message(msg), used))
}

/// The `ServerMessage` header type, re-exported so a caller can name the wire
/// type without depending on `rfb-proto` directly.
pub use rfb_proto::messages::ServerMessage as AuroraServerMessage;

/// Encode a `ServerMessage` into `out` using `rfb-proto`'s writer.
///
/// Used for the framebuffer-update header so the server and the client agree
/// on one writer, not two hand-maintained ones.
pub fn write_server_message(msg: &AuroraServerMessage, out: &mut Vec<u8>) -> Result<(), RfbError> {
    // `ServerMessage::encode` is infallible by construction; it only ever
    // appends to a Vec.
    msg.encode(out);
    Ok(())
}

/// Write a `SecurityResult` with `rfb-proto`'s field writer.
///
/// RFC 6143 7.1.3: the field is a `u32` that is **0 on success** and non-zero
/// on failure, and only a failure carries a reason string. Note the reason is
/// written for every failure, not just when one is supplied: a 3.8 client that
/// selected a rejected security type reads a length-prefixed string here, and
/// omitting it desyncs the stream -- the client then parses reason bytes as
/// the start of `ServerInit`, which presents as a server that authenticated
/// nobody and then went silent.
pub fn write_security_result(out: &mut Vec<u8>, ok: bool, reason: Option<&str>) {
    if ok {
        out.put_u32(0);
        return;
    }
    out.put_u32(1);
    let reason = reason.unwrap_or("authentication failed");
    out.put_u32(reason.len() as u32);
    out.extend_from_slice(reason.as_bytes());
}

// ── Rect ─────────────────────────────────────────────────────────────────────

/// Whether `rfb-proto` recognises an encoding number, so a server can avoid
/// promising a client an encoding whose decoder it does not have.
pub fn is_known_encoding(code: i32) -> bool {
    !matches!(AuroraEncoding::from_code(code), AuroraEncoding::Other(_))
}

/// Build a rectangle header, bounds-checked against the encoder's own limits.
pub fn rect(x: u16, y: u16, width: u16, height: u16, encoding: i32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
        encoding,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_round_trip() {
        for banner in [
            &b"RFB 003.003\n"[..],
            &b"RFB 003.007\n"[..],
            &b"RFB 003.008\n"[..],
        ] {
            let v = parse_client_version(banner).unwrap();
            assert!(decide_version(v).is_ok(), "{banner:?} rejected");
        }
    }

    #[test]
    fn version_above_38_asks_for_retry() {
        assert_eq!(
            decide_version(389).unwrap(),
            VersionDecision::Retry,
            "3.889 must be clamped by asking the peer to retry, not accepted blindly"
        );
    }

    #[test]
    fn version_below_33_refused() {
        assert!(decide_version(302).is_err());
    }

    #[test]
    fn newer_versions_clamp_to_38_message_rules() {
        // rfb-proto clamps 3.889 to the 3.8 message rules, because 3.889 is a
        // real RFC 6143 version whose security handshake *is* the 3.8 one.
        let (v, used) = AuroraVersion::decode(b"RFB 003.889\n").unwrap();
        assert_eq!(used, 12);
        assert_eq!(v, AuroraVersion::V3_8);

        // Note the representation limit this exposes: Continuum encodes a
        // version as major*100 + minor, so 3.889 becomes 1189, not 3889.
        // It is still far above 308, so the retry decision is correct -- but
        // the number is not the one a human would write, and that is a
        // property of the u16 convention, not of the parser.
        let reported = parse_client_version(b"RFB 003.889\n").unwrap();
        assert_eq!(reported, 3 * 100 + 889);
        assert_eq!(decide_version(reported).unwrap(), VersionDecision::Retry);
    }

    #[test]
    fn malformed_version_banner_rejected() {
        for bad in [
            &b"RFB 00x.008\n"[..],
            &b"XXX 003.008\n"[..],
            &b"RFB 003.008"[..], // no trailing newline
        ] {
            assert!(
                parse_client_version(bad).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn pixel_format_round_trips() {
        let fmt = PixelFormat {
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
        let back = from_aurora_pixel_format(&to_aurora_pixel_format(&fmt));
        assert_eq!(back.bits_per_pixel, fmt.bits_per_pixel);
        assert_eq!(back.red_shift, fmt.red_shift);
        assert_eq!(back.blue_shift, fmt.blue_shift);
        assert_eq!(back.true_colour, fmt.true_colour);
    }

    #[test]
    fn pixel_format_encodes_to_16_bytes() {
        let fmt = to_aurora_pixel_format(&PixelFormat {
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
        });
        let mut out = Vec::new();
        fmt.encode(&mut out);
        assert_eq!(out.len(), AuroraPixelFormat::WIRE_LEN);
        let (back, used) = AuroraPixelFormat::decode(&out).unwrap();
        assert_eq!(used, AuroraPixelFormat::WIRE_LEN);
        assert_eq!(back.bits_per_pixel, 32);
    }

    #[test]
    fn server_init_round_trips() {
        let init = ServerInit {
            width: 1280,
            height: 800,
            pixel_format: PixelFormat {
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
            },
            name: "Continuum QEMU".to_string(),
        };
        let mut out = Vec::new();
        to_aurora_server_init(&init).encode(&mut out);
        let (back, used) = AuroraServerInit::decode(&out, &limits()).unwrap();
        assert_eq!(used, out.len());
        let back = from_aurora_server_init(&back);
        assert_eq!(back.width, 1280);
        assert_eq!(back.height, 800);
        assert_eq!(back.name, "Continuum QEMU");
    }

    #[test]
    fn unsupported_security_types_are_filtered() {
        assert!(!is_supported(AuroraSecurityType::from_code(30))); // Apple
        assert!(is_supported(AuroraSecurityType::VncAuth));
        assert!(is_supported(AuroraSecurityType::None));
    }

    #[test]
    fn negotiation_prefers_vnc_auth_over_none() {
        // Client offers both, in the order None, VncAuth.
        let got = negotiate(&[1, 2], &[SecurityType(2), SecurityType(1)]).unwrap();
        assert_eq!(got, SecurityType(2), "preference order must win");
    }

    #[test]
    fn negotiation_refuses_offer_of_only_unimplemented_types() {
        let err = negotiate(&[30], &[SecurityType(2)]).unwrap_err();
        assert!(
            matches!(err, RfbError::Protocol(_)),
            "an unsupported-only offer must be a protocol error, not a downgrade"
        );
    }

    #[test]
    fn negotiation_skips_to_a_supported_type() {
        // Offer None, Apple, VncAuth. Apple is unimplemented and must be
        // stepped over rather than selected.
        let got = negotiate(&[1, 30, 2], &[SecurityType(2)]).unwrap();
        assert_eq!(got, SecurityType(2));
    }

    #[test]
    fn decode_set_encodings_preserves_unknown_numbers() {
        let mut buf = vec![2u8, 0, 0, 3];
        for enc in [0i32, 5, 0x4F_FFFF_FF] {
            buf.extend_from_slice(&enc.to_be_bytes());
        }
        let (msg, used) = decode_client_message(&buf).unwrap();
        assert_eq!(used, buf.len());
        match msg {
            ClientMessage::SetEncodings { encodings } => {
                assert_eq!(encodings, vec![0, 5, 0x4F_FFFF_FF]);
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn decode_key_event() {
        // type 4, down-flag, 2 pad, keysym(4). 8 bytes total.
        let buf = [4u8, 1, 0, 0, 0x00, 0xE0, 0x00, 0x00];
        let (msg, used) = decode_client_message(&buf).unwrap();
        assert_eq!(used, buf.len());
        match msg {
            ClientMessage::KeyEvent { down, keysym } => {
                assert!(down);
                assert_eq!(keysym, 0x00E0_0000);
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn decode_pointer_event() {
        // type 5, button mask, x, y
        let buf = [5u8, 0x01, 0x02, 0x03, 0x04, 0x05];
        let (msg, _) = decode_client_message(&buf).unwrap();
        match msg {
            ClientMessage::PointerEvent { x, y, mask } => {
                assert_eq!((x, y, mask), (0x0203, 0x0405, 0x01));
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn decode_framebuffer_update_request() {
        // type 3, incremental, x(2), y(2), width(2), height(2). 10 bytes.
        let buf = [3u8, 1, 0, 0, 0, 0, 0, 10, 0, 20];
        let (msg, used) = decode_client_message(&buf).unwrap();
        assert_eq!(used, buf.len());
        match msg {
            ClientMessage::FramebufferUpdateRequest {
                incremental,
                x,
                y,
                width,
                height,
            } => {
                assert!(incremental);
                assert_eq!((x, y), (0, 0));
                assert_eq!((width, height), (10, 20));
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn decode_client_cut_text() {
        // type 6, 3 pad, length(4), text. 8 header bytes + 5 of text.
        let mut buf = vec![6u8, 0, 0, 0];
        buf.extend_from_slice(&5u32.to_be_bytes());
        buf.extend_from_slice(b"hello");
        let (msg, used) = decode_client_message(&buf).unwrap();
        assert_eq!(used, buf.len());
        match msg {
            ClientMessage::ClientCutText { text } => assert_eq!(text, "hello"),
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn truncated_message_is_rejected_not_panicked() {
        // A SetPixelFormat header with nothing behind it.
        let buf = [0u8, 0, 0, 0, 0];
        assert!(decode_client_message(&buf).is_err());
    }

    #[test]
    fn empty_buffer_is_rejected() {
        assert!(decode_client_message(&[]).is_err());
    }

    #[test]
    fn known_encodings() {
        assert!(is_known_encoding(0)); // Raw
        assert!(is_known_encoding(5)); // Hextile
        assert!(is_known_encoding(16)); // ZRLE
        assert!(is_known_encoding(-223)); // DesktopSize
        assert!(!is_known_encoding(0x1234_5678));
    }

    #[test]
    fn security_result_omits_reason_on_success() {
        let mut out = Vec::new();
        write_security_result(&mut out, true, Some("ignored"));
        assert_eq!(out.len(), 4, "a successful SecurityResult is just the status");
        assert_eq!(out, vec![0, 0, 0, 0]);
    }

    #[test]
    fn security_result_carries_reason_on_failure() {
        let mut out = Vec::new();
        write_security_result(&mut out, false, Some("bad password"));
        assert_eq!(&out[..4], &[0, 0, 0, 1]);
        let len = u32::from_be_bytes([out[4], out[5], out[6], out[7]]) as usize;
        assert_eq!(&out[8..8 + len], b"bad password");
    }
}
