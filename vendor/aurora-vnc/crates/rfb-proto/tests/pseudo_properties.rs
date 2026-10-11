//! Property tests for the pseudo-encodings, SetDesktopSize and Extended
//! Clipboard: random values round-trip through the encoders and decoders,
//! every proper prefix is Incomplete, and the layout and size rules hold.
//! (The fuzz targets `pseudo_rect` and `ext_clipboard` cover arbitrary bytes.)

use rfb_proto::clipboard::{self, text_from_wire, text_to_wire, ExtClipboard};
use rfb_proto::pseudo::cursor_mask_len;
use rfb_proto::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

const CASES: u64 = 500;

fn screen_in(rng: &mut Rng, id: u32, w: u16, h: u16) -> Screen {
    let x = rng.below(u64::from(w)) as u16;
    let y = rng.below(u64::from(h)) as u16;
    let width = 1 + rng.below(u64::from(w - x)) as u16;
    let height = 1 + rng.below(u64::from(h - y)) as u16;
    Screen { id, x, y, width, height, flags: rng.next() as u32 }
}

fn layout(rng: &mut Rng) -> (u16, u16, Vec<Screen>) {
    let (w, h) = (1 + rng.below(4096) as u16, 1 + rng.below(4096) as u16);
    let n = 1 + rng.below(8) as u32;
    let base = rng.next() as u32;
    (w, h, (0..n).map(|i| screen_in(rng, base.wrapping_add(i), w, h)).collect())
}

fn every_prefix_is_incomplete(what: &str, full: &[u8], decode: impl Fn(&[u8]) -> Result<usize, DecodeError>) {
    assert_eq!(decode(full).unwrap(), full.len(), "{what}: whole");
    for cut in 0..full.len() {
        match decode(&full[..cut]) {
            Err(DecodeError::Incomplete { needed }) => assert!(needed > cut, "{what}: needed {needed} at {cut}"),
            other => panic!("{what}: prefix {cut}/{} gave {other:?}", full.len()),
        }
    }
}

#[test]
fn generated_layouts_are_valid_and_round_trip() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..CASES {
        let (w, h, screens) = layout(&mut rng);
        assert_eq!(validate_layout(w, h, &screens), Ok(()), "{w}x{h} {screens:?}");
        let e = ExtendedDesktopSize {
            reason: rng.below(3) as u16,
            status: rng.below(5) as u16,
            width: w,
            height: h,
            screens,
        };
        let mut out = Vec::new();
        e.encode(&mut out);
        let (hdr, n) = RectHeader::decode(&out).unwrap();
        assert_eq!((n, hdr.encoding, hdr.x, hdr.y), (12, Encoding::ExtendedDesktopSize, e.reason, e.status));
        assert_eq!(ExtendedDesktopSize::decode(&hdr, &out[12..]).unwrap(), (e.clone(), out.len() - 12));
        every_prefix_is_incomplete("eds", &out[12..], |b| ExtendedDesktopSize::decode(&hdr, b).map(|(_, n)| n));

        let m = ClientMessage::SetDesktopSize { width: w, height: h, screens: e.screens.clone() };
        let mut out = Vec::new();
        m.encode(&mut out);
        assert_eq!(out[0], 251);
        assert_eq!(out.len(), 8 + 16 * e.screens.len());
        let limits = Limits::default();
        assert_eq!(ClientMessage::decode(&out, &limits).unwrap(), (m, out.len()));
        every_prefix_is_incomplete("SetDesktopSize", &out, |b| ClientMessage::decode(b, &limits).map(|(_, n)| n));
    }
}

#[test]
fn broken_layouts_are_rejected() {
    let mut rng = Rng(0x5eed_0002);
    for _ in 0..CASES {
        let (w, h, mut screens) = layout(&mut rng);
        let i = rng.below(screens.len() as u64) as usize;
        match rng.below(4) {
            0 => screens[i].width = w - screens[i].x + 1 + rng.below(100) as u16,
            1 => screens[i].height = h - screens[i].y + 1 + rng.below(100) as u16,
            2 => screens[i].width = 0,
            _ if screens.len() > 1 => screens[i].id = screens[(i + 1) % screens.len()].id,
            _ => screens.clear(),
        }
        assert!(validate_layout(w, h, &screens).is_err(), "{w}x{h} {screens:?}");
    }
    assert!(validate_layout(0, 10, &[Screen::whole(0, 0, 10)]).is_err());
    assert!(validate_layout(10, 10, &vec![Screen::whole(0, 10, 10); 256]).is_err());
}

#[test]
fn cursors_round_trip_in_any_pixel_format() {
    let mut rng = Rng(0x5eed_0003);
    let rgb565 = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
        ..PixelFormat::RGBX8888_LE
    };
    let limits = Limits::default();
    for case in 0..CASES {
        let (w, h) = (rng.below(40) as u16, rng.below(40) as u16);
        let (pf, pmask) = if case % 2 == 0 { (PixelFormat::RGBX8888_LE, u32::MAX) } else { (rgb565, 0xffff) };
        let px = usize::from(w) * usize::from(h);
        let row = usize::from(w).div_ceil(8);
        let mut mask = rng.bytes(cursor_mask_len(w, h));
        if w % 8 != 0 {
            // Padding bits at the end of each row are zero on the wire.
            let keep = 0xffu8 << (8 - w % 8);
            mask.chunks_mut(row).for_each(|r| *r.last_mut().unwrap() &= keep);
        }
        let c = Cursor {
            hot_x: if w > 0 { rng.below(u64::from(w)) as u16 } else { 0 },
            hot_y: if h > 0 { rng.below(u64::from(h)) as u16 } else { 0 },
            width: w,
            height: h,
            pixels: (0..px).map(|_| rng.next() as u32 & pmask).collect(),
            mask,
        };
        let mut out = Vec::new();
        c.encode(&pf, &mut out);
        let (hdr, _) = RectHeader::decode(&out).unwrap();
        assert_eq!((hdr.encoding, hdr.x, hdr.y, hdr.width, hdr.height), (Encoding::Cursor, c.hot_x, c.hot_y, w, h));
        assert_eq!(Cursor::decode(&hdr, &pf, &out[12..], &limits).unwrap(), (c.clone(), out.len() - 12));
        every_prefix_is_incomplete("cursor", &out[12..], |b| Cursor::decode(&hdr, &pf, b, &limits).map(|(_, n)| n));

        let a = AlphaCursor { hot_x: c.hot_x, hot_y: c.hot_y, width: w, height: h, rgba: rng.bytes(px * 4) };
        let mut out = Vec::new();
        a.encode(&mut out);
        let (hdr, _) = RectHeader::decode(&out).unwrap();
        assert_eq!(hdr.encoding, Encoding::CursorWithAlpha);
        assert_eq!(AlphaCursor::decode(&hdr, &out[12..], &limits).unwrap(), (a, out.len() - 12));
    }
    // Shapes over the limit are refused before any pixels are read.
    let big = RectHeader { x: 0, y: 0, width: 1025, height: 1, encoding: Encoding::Cursor };
    assert!(matches!(Cursor::decode(&big, &PixelFormat::RGBX8888_LE, &[], &limits), Err(DecodeError::Invalid(_))));
}

fn random_text(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &["a", "Z", " ", "\n", "\r\n", "\r", "é", "ß", "€", "日本", "🦀", "\t", "\u{0}"];
    (0..rng.below(64)).map(|_| PIECES[rng.below(PIECES.len() as u64) as usize]).collect()
}

#[test]
fn extended_clipboard_messages_round_trip() {
    let mut rng = Rng(0x5eed_0004);
    let max = 1 << 20;
    for _ in 0..CASES {
        let formats = (rng.next() as u32 & 0x1f) | clipboard::TEXT;
        let m = match rng.below(5) {
            0 => {
                let sizes = (0..formats.count_ones()).map(|_| rng.next() as u32).collect();
                ExtClipboard::Caps { flags: formats | (rng.next() as u32 & 0x1f00_0000 & !clipboard::CAPS), sizes }
            }
            1 => ExtClipboard::Request { formats },
            2 => ExtClipboard::Peek,
            3 => ExtClipboard::Notify { formats },
            _ => {
                let items = (0..5)
                    .map(|b| 1u32 << b)
                    .filter(|f| formats & f != 0)
                    .map(|f| {
                        let n = rng.below(300) as usize;
                        (f, rng.bytes(n))
                    })
                    .collect();
                ExtClipboard::Provide { items }
            }
        };
        let mut body = Vec::new();
        m.encode_body(&mut body);
        assert_eq!(ExtClipboard::decode_body(&body, max).unwrap(), m);
        assert!(body.len() as u32 <= ExtClipboard::max_body_len(max) || matches!(m, ExtClipboard::Caps { .. }));

        // As server and client messages: a negative length, then the body.
        for server in [false, true] {
            let mut out = Vec::new();
            if server {
                ServerMessage::ExtendedClipboard(m.clone()).encode(&mut out);
            } else {
                ClientMessage::ExtendedClipboard(m.clone()).encode(&mut out);
            }
            assert_eq!(i32::from_be_bytes(out[4..8].try_into().unwrap()), -(body.len() as i32));
            assert_eq!(out[8..], body[..]);
            let limits = Limits::default();
            let n = if server {
                let (got, n) = ServerMessage::decode(&out, &limits).unwrap();
                assert_eq!(got, ServerMessage::ExtendedClipboard(m.clone()));
                n
            } else {
                let (got, n) = ClientMessage::decode(&out, &limits).unwrap();
                assert_eq!(got, ClientMessage::ExtendedClipboard(m.clone()));
                n
            };
            assert_eq!(n, out.len());
        }
    }
}

#[test]
fn clipboard_text_survives_the_wire() {
    let mut rng = Rng(0x5eed_0005);
    for _ in 0..CASES {
        let text = random_text(&mut rng);
        let wire = text_to_wire(&text);
        assert_eq!(wire.last(), Some(&0), "NUL-terminated");
        assert!(!wire[..wire.len() - 1].contains(&0), "no embedded NUL");
        assert!(!wire.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'), "line ends are CRLF: {wire:?}");
        // Line ends normalise to LF; a NUL ends the text.
        let want = text.split('\0').next().unwrap().replace("\r\n", "\n").replace('\r', "\n");
        assert_eq!(text_from_wire(&wire), want);
        let p = ExtClipboard::provide_text(&text);
        assert_eq!(p.text().as_deref(), Some(want.as_str()));
    }
}

#[test]
fn provide_limits_apply_after_decompression() {
    let mut rng = Rng(0x5eed_0006);
    for _ in 0..50 {
        let max = 1024 + rng.below(1 << 16) as u32;
        let len = max as usize + 1 + rng.below(1 << 16) as usize;
        let mut body = Vec::new();
        // Zeros compress about 1000:1, so the body itself is tiny.
        ExtClipboard::Provide { items: vec![(clipboard::TEXT, vec![0; len])] }.encode_body(&mut body);
        assert!(body.len() < max as usize);
        assert!(matches!(ExtClipboard::decode_body(&body, max), Err(DecodeError::Invalid(_))), "{len} > {max}");
        let mut ok = Vec::new();
        ExtClipboard::Provide { items: vec![(clipboard::TEXT, vec![0; max as usize])] }.encode_body(&mut ok);
        assert!(ExtClipboard::decode_body(&ok, max).is_ok());
    }
}
