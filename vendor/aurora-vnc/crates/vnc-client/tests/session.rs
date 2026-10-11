//! Sans-I/O tests: the client `Session` talks to rfb-proto's `ServerHandshake`
//! in memory, then to hand-built server messages. No sockets, fully deterministic.

use rfb_proto::{
    ClientMessage, Encoding, Limits, PixelFormat, RectHeader, SecurityType, ServerConfig, ServerHandshake, ServerInit,
    ServerMessage, Version,
};
use vnc_client::{ClientError, Event, Rect, Session, SessionOptions};

const W: u16 = 8;
const H: u16 = 4;

fn server(pf: PixelFormat) -> ServerHandshake {
    ServerHandshake::new(ServerConfig {
        version: Version::V3_8,
        security: vec![SecurityType::None],
        server_init: ServerInit { width: W, height: H, pixel_format: pf, name: b"test-desk".to_vec() },
        vnc_auth: None,
        vencrypt: None,
        limits: Limits::default(),
    })
}

/// Runs the handshake in memory and returns the connected session plus the
/// bytes the client sent after the handshake (SetPixelFormat, SetEncodings, ...).
fn connect(opts: SessionOptions, pf: PixelFormat) -> (Session, Vec<u8>, Vec<Event>) {
    let mut c = Session::new(opts);
    let mut s = server(pf);
    let mut to_server = Vec::new();
    let mut events = Vec::new();
    for _ in 0..16 {
        let from_server = s.take_output();
        events.extend(c.receive(&from_server).unwrap());
        to_server.extend(c.take_output());
        if c.is_connected() {
            break;
        }
        if !s.is_done() {
            let used = s.receive(&to_server).unwrap();
            to_server.drain(..used);
        }
    }
    assert!(c.is_connected(), "handshake did not complete");
    (c, to_server, events)
}

fn decode_all(mut b: &[u8]) -> Vec<ClientMessage> {
    let mut v = Vec::new();
    while !b.is_empty() {
        let (m, n) = ClientMessage::decode(b, &Limits::default()).unwrap();
        v.push(m);
        b = &b[n..];
    }
    v
}

fn raw_rect(r: Rect, px: impl Fn(u16, u16) -> u32) -> Vec<u8> {
    let mut out = Vec::new();
    RectHeader { x: r.x, y: r.y, width: r.width, height: r.height, encoding: Encoding::Raw }.encode(&mut out);
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            out.extend_from_slice(&px(x, y).to_le_bytes());
        }
    }
    out
}

fn update(rects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    ServerMessage::FramebufferUpdate { num_rects: rects.len() as u16 }.encode(&mut out);
    for r in rects {
        out.extend_from_slice(r);
    }
    out
}

#[test]
fn handshake_then_setup_messages() {
    let (_c, sent, events) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    assert_eq!(events, vec![Event::Connected { width: W, height: H, name: "test-desk".into() }]);
    let msgs = decode_all(&sent);
    assert_eq!(msgs[0], ClientMessage::SetPixelFormat(PixelFormat::RGBX8888_LE));
    assert_eq!(
        msgs[1],
        ClientMessage::SetEncodings(vec![
            Encoding::CopyRect,
            Encoding::Zrle,
            Encoding::Tight,
            Encoding::Raw,
            Encoding::ExtendedDesktopSize,
            Encoding::DesktopSize,
            Encoding::Cursor,
            Encoding::ExtendedClipboard,
        ])
    );
    assert_eq!(
        msgs[2],
        ClientMessage::FramebufferUpdateRequest { incremental: false, x: 0, y: 0, width: W, height: H }
    );
}

#[test]
fn unsupported_encodings_are_never_advertised() {
    let opts = SessionOptions { encodings: vec![Encoding::Hextile, Encoding::Trle], ..Default::default() };
    let (_c, sent, _) = connect(opts, PixelFormat::RGBX8888_LE);
    assert!(decode_all(&sent).contains(&ClientMessage::SetEncodings(vec![Encoding::Raw])));
}

#[test]
fn raw_update_fed_one_byte_at_a_time() {
    let opts = SessionOptions { continuous_updates: false, ..Default::default() };
    let (mut c, _, _) = connect(opts, PixelFormat::RGBX8888_LE);
    let full = Rect { x: 0, y: 0, width: W, height: H };
    let msg = update(&[raw_rect(full, |x, y| ((x as u32) << 16) | ((y as u32) << 8) | 0x5a)]);
    let mut events = Vec::new();
    for b in &msg {
        events.extend(c.receive(std::slice::from_ref(b)).unwrap());
    }
    assert_eq!(events, vec![Event::Updated { damage: vec![full] }]);
    let fb = c.framebuffer();
    assert_eq!(fb.pixel(3, 2), 0x03025a);
    assert_eq!(fb.pixel(7, 3), 0x07035a);
}

#[test]
fn continuous_mode_requests_next_incremental_update() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    c.receive(&update(&[])).unwrap();
    assert_eq!(
        decode_all(&c.take_output()),
        vec![ClientMessage::FramebufferUpdateRequest { incremental: true, x: 0, y: 0, width: W, height: H }]
    );
}

#[test]
fn copyrect_and_desktop_size() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    let full = Rect { x: 0, y: 0, width: W, height: H };
    c.receive(&update(&[raw_rect(full, |x, _| x as u32)])).unwrap();

    let mut cr = Vec::new();
    RectHeader { x: 4, y: 0, width: 4, height: H, encoding: Encoding::CopyRect }.encode(&mut cr);
    cr.extend_from_slice(&[0, 0, 0, 0]);
    c.receive(&update(&[cr])).unwrap();
    assert_eq!(c.framebuffer().pixel(5, 1), 1);

    let mut ds = Vec::new();
    RectHeader { x: 0, y: 0, width: 16, height: 2, encoding: Encoding::DesktopSize }.encode(&mut ds);
    let ev = c.receive(&update(&[ds])).unwrap();
    assert!(ev.contains(&Event::Resized { width: 16, height: 2 }));
    assert_eq!((c.framebuffer().width(), c.framebuffer().height()), (16, 2));
    assert_eq!(c.framebuffer().pixel(5, 1), 1, "overlap kept across resize");
}

#[test]
fn cursor_shape_is_an_event_not_damage() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    // 2x1 cursor, hotspot (1,0): red opaque, then a transparent pixel.
    let mut cur = Vec::new();
    RectHeader { x: 1, y: 0, width: 2, height: 1, encoding: Encoding::Cursor }.encode(&mut cur);
    cur.extend_from_slice(&[0, 0, 0xff, 0, 0xff, 0xff, 0xff, 0, 0b1000_0000]);
    // Fed one byte at a time, so the payload length must come out right.
    let msg = update(&[cur]);
    let mut events = Vec::new();
    for b in &msg {
        events.extend(c.receive(std::slice::from_ref(b)).unwrap());
    }
    let shape = events
        .iter()
        .find_map(|e| match e {
            Event::CursorShape(s) => Some(s.clone()),
            _ => None,
        })
        .expect("cursor event");
    assert_eq!((shape.width, shape.height, shape.hot_x, shape.hot_y), (2, 1, 1, 0));
    assert_eq!(shape.argb, vec![0xffff_0000, 0]);
    assert_eq!(c.cursor(), Some(&shape));
    assert!(events.contains(&Event::Updated { damage: vec![] }));

    let mut huge = Vec::new();
    RectHeader { x: 0, y: 0, width: 4096, height: 4096, encoding: Encoding::Cursor }.encode(&mut huge);
    assert!(matches!(c.receive(&update(&[huge])), Err(ClientError::Protocol(_))));
}

#[test]
fn extended_desktop_size_resize_flow() {
    use vnc_client::rfb_proto::{ExtendedDesktopSize, Screen};
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    assert!(!c.request_desktop_size(100, 100), "no resize before the server offers it");
    let eds = |reason, status, w, h| {
        let mut out = Vec::new();
        ExtendedDesktopSize { reason, status, width: w, height: h, screens: vec![Screen::whole(42, w, h)] }
            .encode(&mut out);
        out
    };
    // The server's answer to our first request: same size, now resizable.
    let ev = c.receive(&update(&[eds(0, 0, W, H)])).unwrap();
    assert!(!ev.iter().any(|e| matches!(e, Event::Resized { .. })));
    assert!(c.can_resize());
    let _ = c.take_output();

    assert!(c.request_desktop_size(32, 16));
    assert!(!c.request_desktop_size(W, H), "already that size");
    assert_eq!(
        decode_all(&c.take_output()),
        vec![ClientMessage::SetDesktopSize { width: 32, height: 16, screens: vec![Screen::whole(42, 32, 16)] }]
    );
    let ev = c.receive(&update(&[eds(1, 0, 32, 16)])).unwrap();
    assert!(ev.contains(&Event::Resized { width: 32, height: 16 }));
    assert_eq!((c.framebuffer().width(), c.framebuffer().height()), (32, 16));

    let ev = c.receive(&update(&[eds(1, 1, 32, 16)])).unwrap();
    assert!(ev.iter().any(|e| matches!(e, Event::ResizeRefused { status: 1, .. })));

    // A layout with a screen outside the framebuffer is a protocol error.
    let mut bad = Vec::new();
    ExtendedDesktopSize { reason: 0, status: 0, width: 10, height: 10, screens: vec![Screen::whole(1, 20, 10)] }
        .encode(&mut bad);
    assert!(matches!(c.receive(&update(&[bad])), Err(ClientError::Protocol(_))));
}

#[test]
fn out_of_bounds_rect_is_fatal_and_session_stays_closed() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    let bad = raw_rect(Rect { x: W - 1, y: 0, width: 2, height: 1 }, |_, _| 0);
    assert!(matches!(c.receive(&update(&[bad])), Err(ClientError::Protocol(_))));
    assert_eq!(c.receive(&[]), Err(ClientError::Closed));
}

#[test]
fn unnegotiated_encoding_is_fatal() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    let mut r = Vec::new();
    RectHeader { x: 0, y: 0, width: 1, height: 1, encoding: Encoding::Hextile }.encode(&mut r);
    assert!(c.receive(&update(&[r])).is_err());
}

#[test]
fn huge_framebuffer_refused() {
    let opts = SessionOptions { max_framebuffer_pixels: 10, ..Default::default() };
    let mut c = Session::new(opts);
    let mut s = server(PixelFormat::RGBX8888_LE);
    let mut err = None;
    for _ in 0..16 {
        match c.receive(&s.take_output()) {
            Ok(_) => {}
            Err(e) => {
                err = Some(e);
                break;
            }
        }
        let out = c.take_output();
        if !s.is_done() {
            s.receive(&out).unwrap();
        }
    }
    assert!(matches!(err, Some(ClientError::Protocol(_))));
}

#[test]
fn bell_and_cut_text_events() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    let mut b = Vec::new();
    ServerMessage::Bell.encode(&mut b);
    ServerMessage::ServerCutText(b"hello".to_vec()).encode(&mut b);
    assert_eq!(c.receive(&b).unwrap(), vec![Event::Bell, Event::ServerCutText(b"hello".to_vec())]);
}

#[test]
fn native_16bpp_format_when_not_overridden() {
    let pf = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: false,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };
    let opts = SessionOptions { pixel_format: None, ..Default::default() };
    let (mut c, sent, _) = connect(opts, pf);
    assert!(!decode_all(&sent).iter().any(|m| matches!(m, ClientMessage::SetPixelFormat(_))));
    let mut r = Vec::new();
    RectHeader { x: 0, y: 0, width: 1, height: 1, encoding: Encoding::Raw }.encode(&mut r);
    r.extend_from_slice(&0xf800u16.to_le_bytes());
    c.receive(&update(&[r])).unwrap();
    assert_eq!(c.framebuffer().pixel(0, 0), 0xff0000);
}

#[test]
fn input_messages() {
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    c.key(0xff0d, true);
    c.pointer(1000, 2, 1);
    c.scroll(1, 1, true);
    c.cut_text(b"x");
    assert_eq!(
        decode_all(&c.take_output()),
        vec![
            ClientMessage::KeyEvent { down: true, keysym: 0xff0d },
            ClientMessage::PointerEvent { button_mask: 1, x: W - 1, y: 2 },
            ClientMessage::PointerEvent { button_mask: 1 | 8, x: 1, y: 1 },
            ClientMessage::PointerEvent { button_mask: 1, x: 1, y: 1 },
            ClientMessage::ClientCutText(b"x".to_vec()),
        ]
    );
}

#[test]
fn extended_clipboard_utf8_both_ways() {
    use rfb_proto::clipboard::{self as xc, ExtClipboard};
    let (mut c, _, _) = connect(SessionOptions::default(), PixelFormat::RGBX8888_LE);
    c.take_output();
    assert!(!c.clipboard_extended());
    let server = |m: ExtClipboard| {
        let mut b = Vec::new();
        ServerMessage::ExtendedClipboard(m).encode(&mut b);
        b
    };
    let caps = ExtClipboard::Caps { flags: xc::TEXT | xc::REQUEST | xc::NOTIFY | xc::PROVIDE, sizes: vec![1 << 20] };
    assert!(c.receive(&server(caps)).unwrap().is_empty());
    assert!(c.clipboard_extended());
    assert!(matches!(&decode_all(&c.take_output())[..], [ClientMessage::ExtendedClipboard(ExtClipboard::Caps { .. })]));

    // Ours: Notify, then the server's Request gets the UTF-8 text.
    c.set_clipboard("Grüße, мир, 世界");
    assert_eq!(
        decode_all(&c.take_output()),
        vec![ClientMessage::ExtendedClipboard(ExtClipboard::Notify { formats: xc::TEXT })]
    );
    c.receive(&server(ExtClipboard::Request { formats: xc::TEXT })).unwrap();
    let sent = decode_all(&c.take_output());
    let [ClientMessage::ExtendedClipboard(p)] = &sent[..] else { panic!("{sent:?}") };
    assert_eq!(p.text().as_deref(), Some("Grüße, мир, 世界"));

    // Theirs: 2 MiB of text arrives intact.
    let big: String = "αβγ\n".repeat(1 << 18);
    let ev = c.receive(&server(ExtClipboard::provide_text(&big))).unwrap();
    assert_eq!(ev, vec![Event::ClipboardText(big)]);
}

/// TightVNC's first update carries only a size change and the cursor. The
/// client must not treat that as a picture: it asks for a full refresh at
/// the new size (even without continuous updates) and reports no pixels yet.
#[test]
fn size_only_update_triggers_one_full_refresh() {
    let opts = SessionOptions { continuous_updates: false, ..Default::default() };
    let (mut c, _, _) = connect(opts, PixelFormat::RGBX8888_LE);
    c.take_output();
    let mut ds = Vec::new();
    RectHeader { x: 0, y: 0, width: 12, height: 3, encoding: Encoding::DesktopSize }.encode(&mut ds);
    let mut cur = Vec::new();
    RectHeader { x: 0, y: 0, width: 1, height: 1, encoding: Encoding::Cursor }.encode(&mut cur);
    cur.extend_from_slice(&[0, 0, 0xff, 0, 0b1000_0000]);
    c.receive(&update(&[ds, cur])).unwrap();
    assert!(!c.has_pixels(), "a size-only update is not a picture");
    assert_eq!(
        decode_all(&c.take_output()),
        vec![ClientMessage::FramebufferUpdateRequest { incremental: false, x: 0, y: 0, width: 12, height: 3 }]
    );

    let full = Rect { x: 0, y: 0, width: 12, height: 3 };
    c.receive(&update(&[raw_rect(full, |x, _| x as u32)])).unwrap();
    assert!(c.has_pixels());
    assert!(c.take_output().is_empty(), "only one refresh per resize, and none in one-shot mode");
}
