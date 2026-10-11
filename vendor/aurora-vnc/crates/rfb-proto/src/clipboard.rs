//! The Extended Clipboard pseudo-encoding (0xC0A1E5CE) from the community
//! RFB specification, as implemented by TigerVNC.
//!
//! Once negotiated, a ClientCutText or ServerCutText whose length (read as
//! s32) is negative carries an [`ExtClipboard`] body of `-length` bytes:
//! a u32 of flags (format bits 0-15, action bits 24-28) and then
//! action-specific data. Text is UTF-8 with CRLF line endings and a
//! terminating NUL; see [`text_to_wire`] and [`text_from_wire`].
//!
//! `Provide` data is a zlib stream of `(u32 size, bytes)` per format, in
//! ascending bit order. Each message uses a fresh stream (TigerVNC
//! sync-flushes rather than finishing it, and both forms are accepted).
//! The decoder's limit, `Limits::max_cut_text_len`, applies to the
//! *decompressed* total, so a small zip bomb is refused before it inflates.

use crate::codec::{len_u32, DecodeError, PutBe, Reader};
use crate::encodings::zlib::{Deflater, Inflater};

/// Format bits.
pub const TEXT: u32 = 1 << 0;
pub const RTF: u32 = 1 << 1;
pub const HTML: u32 = 1 << 2;
pub const DIB: u32 = 1 << 3;
pub const FILES: u32 = 1 << 4;
/// All 16 format bits.
pub const FORMAT_MASK: u32 = 0xffff;

/// Action bits.
pub const CAPS: u32 = 1 << 24;
pub const REQUEST: u32 = 1 << 25;
pub const PEEK: u32 = 1 << 26;
pub const NOTIFY: u32 = 1 << 27;
pub const PROVIDE: u32 = 1 << 28;
pub const ACTION_MASK: u32 = 0x1f00_0000;

/// What a peer that never sent Caps is assumed to support: text, RTF and
/// HTML; request, notify and provide; text up to 20 MiB, RTF and HTML none
/// (rfbproto section "Extended Clipboard Pseudo-Encoding").
pub const DEFAULT_PEER_FLAGS: u32 = TEXT | RTF | HTML | REQUEST | NOTIFY | PROVIDE;
pub const DEFAULT_PEER_TEXT_SIZE: u32 = 20 * 1024 * 1024;

/// One Extended Clipboard message body. Formats are bitmasks of the format
/// bits above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtClipboard {
    /// Supported formats and actions (`flags`; [`CAPS`] itself is implied
    /// and stripped when decoding) and the
    /// largest unsolicited [`Provide`](Self::Provide) accepted per format,
    /// one size per set format bit in ascending order.
    Caps { flags: u32, sizes: Vec<u32> },
    /// Asks the peer to provide these formats.
    Request { formats: u32 },
    /// Asks the peer to notify which formats it has.
    Peek,
    /// The sender's clipboard now holds these formats (0 = empty).
    Notify { formats: u32 },
    /// Clipboard contents: (format bit, bytes) in ascending bit order.
    Provide { items: Vec<(u32, Vec<u8>)> },
    /// No action bit we understand; kept so it can be logged and ignored.
    Other { flags: u32 },
}

impl ExtClipboard {
    /// A Provide holding only UTF-8 text (converted with [`text_to_wire`]).
    pub fn provide_text(text: &str) -> Self {
        ExtClipboard::Provide { items: vec![(TEXT, text_to_wire(text))] }
    }

    /// The text item of a Provide, decoded with [`text_from_wire`].
    pub fn text(&self) -> Option<String> {
        match self {
            ExtClipboard::Provide { items } => items.iter().find(|(f, _)| *f == TEXT).map(|(_, b)| text_from_wire(b)),
            _ => None,
        }
    }

    /// The flags word this message starts with.
    pub fn flags(&self) -> u32 {
        match self {
            ExtClipboard::Caps { flags, .. } => *flags | CAPS,
            ExtClipboard::Request { formats } => REQUEST | (formats & FORMAT_MASK),
            ExtClipboard::Peek => PEEK,
            ExtClipboard::Notify { formats } => NOTIFY | (formats & FORMAT_MASK),
            ExtClipboard::Provide { items } => PROVIDE | items.iter().fold(0, |a, (f, _)| a | f),
            ExtClipboard::Other { flags } => *flags,
        }
    }

    /// The body (flags and data), without the cut-text header.
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        out.put_u32(self.flags());
        match self {
            ExtClipboard::Caps { flags, sizes } => {
                assert_eq!(sizes.len(), (flags & FORMAT_MASK).count_ones() as usize, "one size per format");
                for &s in sizes {
                    out.put_u32(s);
                }
            }
            ExtClipboard::Provide { items } => {
                let mut plain = Vec::new();
                let mut last = 0u32;
                for (f, data) in items {
                    assert!(f.is_power_of_two() && f & FORMAT_MASK != 0 && *f > last, "formats must ascend");
                    last = *f;
                    plain.put_u32(len_u32(data.len(), "clipboard data"));
                    plain.extend_from_slice(data);
                }
                Deflater::new(6).compress(&plain, out);
            }
            _ => {}
        }
    }

    /// Decodes a complete body. `max_len` bounds every format's size in
    /// Caps-free use and the total decompressed size of a Provide.
    pub fn decode_body(body: &[u8], max_len: u32) -> Result<Self, DecodeError> {
        let invalid = |m: String| DecodeError::Invalid(m);
        let mut r = Reader::new(body);
        let flags = r.u32().map_err(|_| invalid("extended clipboard body shorter than 4 bytes".into()))?;
        let formats = flags & FORMAT_MASK;
        if flags & CAPS != 0 {
            let n = formats.count_ones() as usize;
            let sizes = (0..n)
                .map(|_| r.u32())
                .collect::<Result<_, _>>()
                .map_err(|_| invalid("extended clipboard caps truncated".into()))?;
            return Ok(ExtClipboard::Caps { flags: flags & !CAPS, sizes });
        }
        if flags & PROVIDE != 0 {
            let max_out = (max_len as usize).saturating_add(4 * formats.count_ones() as usize);
            let plain = Inflater::new().inflate(&body[4..], max_out)?;
            let mut p = Reader::new(&plain);
            let mut items = Vec::new();
            let mut total = 0u64;
            for bit in 0..16 {
                let f = 1u32 << bit;
                if formats & f == 0 {
                    continue;
                }
                let size = p.u32().map_err(|_| invalid(format!("clipboard provide ends before format {f:#x}")))?;
                total += u64::from(size);
                if total > u64::from(max_len) {
                    return Err(invalid(format!("clipboard provide exceeds the {max_len}-byte limit")));
                }
                let data = p
                    .bytes(size as usize)
                    .map_err(|_| invalid(format!("clipboard provide truncated in format {f:#x}")))?;
                items.push((f, data.to_vec()));
            }
            return Ok(ExtClipboard::Provide { items });
        }
        Ok(if flags & REQUEST != 0 {
            ExtClipboard::Request { formats }
        } else if flags & PEEK != 0 {
            ExtClipboard::Peek
        } else if flags & NOTIFY != 0 {
            ExtClipboard::Notify { formats }
        } else {
            ExtClipboard::Other { flags }
        })
    }

    /// Largest body accepted on the wire for a decompressed limit of `max_len`:
    /// incompressible data grows slightly under deflate.
    pub fn max_body_len(max_len: u32) -> u32 {
        max_len.saturating_add(max_len / 64).saturating_add(4096)
    }
}

/// UTF-8 text as carried by Extended Clipboard: LF becomes CRLF (existing
/// CRLF pairs are kept), followed by a terminating NUL. Text after an
/// embedded NUL is dropped, since the peer would stop reading there.
pub fn text_to_wire(text: &str) -> Vec<u8> {
    let text = text.split('\0').next().unwrap_or("");
    let mut out = Vec::with_capacity(text.len() + 1);
    let mut prev = 0u8;
    for &b in text.as_bytes() {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out.push(0);
    out
}

/// The inverse of [`text_to_wire`]: stops at the first NUL, turns CRLF and
/// lone CR into LF, and replaces invalid UTF-8.
pub fn text_from_wire(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]);
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(m: &ExtClipboard) -> ExtClipboard {
        let mut b = Vec::new();
        m.encode_body(&mut b);
        ExtClipboard::decode_body(&b, 1 << 20).unwrap()
    }

    #[test]
    fn every_action_round_trips() {
        for m in [
            ExtClipboard::Caps { flags: TEXT | HTML | REQUEST | PROVIDE, sizes: vec![1000, 0] },
            ExtClipboard::Request { formats: TEXT },
            ExtClipboard::Peek,
            ExtClipboard::Notify { formats: 0 },
            ExtClipboard::Notify { formats: TEXT | RTF },
            ExtClipboard::provide_text("héllo\nwörld ✓"),
            ExtClipboard::Provide { items: vec![(TEXT, b"a\0".to_vec()), (HTML, b"<b>a</b>".to_vec())] },
            ExtClipboard::Other { flags: 1 << 30 },
        ] {
            assert_eq!(round_trip(&m), m);
        }
        assert_eq!(ExtClipboard::provide_text("a\r\nb\nc").text().unwrap(), "a\nb\nc");
    }

    #[test]
    fn text_wire_form_matches_tigervnc() {
        assert_eq!(text_to_wire("a\nb\r\n"), b"a\r\nb\r\n\0");
        assert_eq!(text_from_wire(b"x\r\ny\0garbage"), "x\ny");
        assert_eq!(text_from_wire(b"no nul"), "no nul");
    }

    #[test]
    fn the_limit_applies_to_decompressed_size() {
        // 1 MiB of zeros compresses to about a kilobyte.
        let bomb = ExtClipboard::Provide { items: vec![(TEXT, vec![0; 1 << 20])] };
        let mut b = Vec::new();
        bomb.encode_body(&mut b);
        assert!(b.len() < 4096);
        assert!(matches!(ExtClipboard::decode_body(&b, 64 * 1024), Err(DecodeError::Invalid(_))));
        assert_eq!(ExtClipboard::decode_body(&b, 1 << 20).unwrap(), bomb);
    }

    #[test]
    fn malformed_bodies_are_invalid() {
        for body in [&[0u8, 0][..], &(CAPS | TEXT).to_be_bytes(), &(PROVIDE | TEXT).to_be_bytes()] {
            assert!(matches!(ExtClipboard::decode_body(body, 1024), Err(DecodeError::Invalid(_))), "{body:?}");
        }
    }
}
