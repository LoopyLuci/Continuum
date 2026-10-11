//! ExtendedDesktopSize (rfbproto, pseudo-encoding -308), parsed independently
//! of `rfb_proto`'s codec so the two cross-check each other.
//!
//! Rectangle: x = reason, y = status, width/height = new framebuffer size.
//! Payload: u8 screen count, 3 padding bytes, then per screen
//! u32 id, u16 x, u16 y, u16 width, u16 height, u32 flags (16 bytes).

use rfb_proto::Screen;

/// x of the rectangle: who caused the change.
pub const REASON_THIS_CLIENT: u16 = 1;
/// y of the rectangle when the change worked.
pub const STATUS_OK: u16 = 0;

/// Payload length, or `None` until the count byte has arrived.
pub fn payload_len(buf: &[u8]) -> Option<usize> {
    buf.first().map(|&n| 4 + n as usize * 16)
}

/// Parses a complete payload of [`payload_len`] bytes.
pub fn parse(buf: &[u8]) -> Result<Vec<Screen>, String> {
    let n = *buf.first().ok_or("empty ExtendedDesktopSize payload")? as usize;
    if buf.len() != 4 + n * 16 {
        return Err(format!("ExtendedDesktopSize payload is {} bytes, expected {}", buf.len(), 4 + n * 16));
    }
    let be16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]);
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Ok(buf[4..]
        .chunks_exact(16)
        .map(|s| Screen {
            id: be32(&s[0..]),
            x: be16(&s[4..]),
            y: be16(&s[6..]),
            width: be16(&s[8..]),
            height: be16(&s[10..]),
            flags: be32(&s[12..]),
        })
        .collect())
}

/// Human-readable reason a server refused a resize (the rectangle's y).
pub fn status_text(status: u16) -> &'static str {
    match status {
        1 => "resizing is not allowed",
        2 => "the server is out of resources",
        3 => "the server rejected the screen layout",
        _ => "the server refused the resize",
    }
}

/// The layout to request for a `width x height` framebuffer: the current
/// screens collapsed into one covering everything, keeping the first id and
/// its flags so the server sees it as the same monitor resized.
pub fn single_screen(current: &[Screen], width: u16, height: u16) -> Vec<Screen> {
    let (id, flags) = current.first().map_or((0, 0), |s| (s.id, s.flags));
    vec![Screen { id, x: 0, y: 0, width, height, flags }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rfb_proto::{Encoding, ExtendedDesktopSize, RectHeader};

    #[test]
    fn matches_rfb_proto_encoder() {
        let screens = vec![
            Screen { id: 7, x: 0, y: 0, width: 1920, height: 1080, flags: 0 },
            Screen { id: 0xdead_beef, x: 1920, y: 100, width: 1280, height: 800, flags: 0x8000_0001 },
        ];
        let eds = ExtendedDesktopSize { reason: 2, status: 0, width: 3200, height: 1080, screens: screens.clone() };
        let mut out = Vec::new();
        eds.encode(&mut out);
        let (h, hl) = RectHeader::decode(&out).unwrap();
        assert_eq!((h.x, h.y, h.width, h.height, h.encoding), (2, 0, 3200, 1080, Encoding::ExtendedDesktopSize));
        let payload = &out[hl..];
        assert_eq!(payload_len(payload), Some(payload.len()));
        for cut in 0..payload.len() {
            assert!(payload_len(&payload[..cut]).is_none_or(|n| n > cut), "prefix {cut}");
        }
        assert_eq!(parse(payload).unwrap(), screens);
    }

    #[test]
    fn bad_payloads() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[1, 0, 0, 0]).is_err());
        assert_eq!(parse(&[0, 0, 0, 0]).unwrap(), vec![]);
    }

    #[test]
    fn single_screen_keeps_identity() {
        let cur = [Screen { id: 9, x: 5, y: 5, width: 10, height: 10, flags: 3 }];
        assert_eq!(
            single_screen(&cur, 800, 600),
            vec![Screen { id: 9, x: 0, y: 0, width: 800, height: 600, flags: 3 }]
        );
        assert_eq!(single_screen(&[], 8, 6)[0].id, 0);
    }
}
