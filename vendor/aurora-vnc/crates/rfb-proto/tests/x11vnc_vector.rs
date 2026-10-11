//! Checks rfb-proto against the byte-level transcript Squish's interop suite
//! captured from a real x11vnc server (crates/interop/vectors).

use rfb_proto::*;

struct Vector {
    /// (from_server, bytes) per transcript line, in order.
    lines: Vec<(bool, Vec<u8>)>,
    /// Bytes elided from the final line by the "...(+N bytes)" marker.
    elided: usize,
}

fn load() -> Vector {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../interop/vectors/x11vnc-rfb38-none-raw.txt");
    let text = std::fs::read_to_string(path).expect("vector file");
    let mut lines = Vec::new();
    let mut elided = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut toks = line.split_whitespace();
        let dir = toks.next().unwrap();
        let mut bytes = Vec::new();
        for t in toks {
            if let Some(rest) = t.strip_prefix("...(+") {
                elided = rest.trim_end_matches("bytes)").trim().parse().unwrap_or(0);
                break;
            }
            if t.starts_with("bytes)") {
                break;
            }
            bytes.push(u8::from_str_radix(t, 16).unwrap_or_else(|_| panic!("bad hex token {t:?}")));
        }
        lines.push((dir == "S", bytes));
    }
    // The marker may be split across tokens: "...(+255936" "bytes)".
    if elided == 0 {
        if let Some(i) = text.find("...(+") {
            let digits: String = text[i + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
            elided = digits.parse().unwrap_or(0);
        }
    }
    Vector { lines, elided }
}

fn side(v: &Vector, server: bool) -> Vec<Vec<u8>> {
    v.lines.iter().filter(|(s, _)| *s == server).map(|(_, b)| b.clone()).collect()
}

#[test]
fn client_handshake_reproduces_x11vnc_session() {
    let v = load();
    let server_lines = side(&v, true);
    let client_lines = side(&v, false);

    // Feed the server's bytes one transcript line at a time, the way they
    // arrived on the socket, and collect what the client state machine says.
    let mut hs = ClientHandshake::new(ClientConfig::default());
    let mut pending = Vec::new();
    let mut client_out = Vec::new();
    let mut lines = server_lines.iter();
    while !hs.is_done() {
        pending.extend_from_slice(lines.next().expect("ran out of server bytes"));
        let used = hs.receive(&pending).expect("handshake");
        pending.drain(..used);
        client_out.extend(hs.take_output());
    }
    assert!(pending.is_empty());
    assert_eq!(hs.version(), Some(Version::V3_8));

    let expected: Vec<u8> = client_lines[..3].concat();
    assert_eq!(client_out, expected, "client handshake bytes differ from what x11vnc accepted");

    let si = hs.server_init().unwrap().clone();
    assert_eq!((si.width, si.height), (320, 200));
    assert_eq!(si.pixel_format, PixelFormat::RGBX8888_LE);

    // The normal-phase client messages the interop client sent.
    let limits = Limits::default();
    let mut buf = Vec::new();
    ClientMessage::SetEncodings(vec![Encoding::Raw]).encode(&mut buf);
    assert_eq!(buf, client_lines[3]);
    buf.clear();
    let req = ClientMessage::FramebufferUpdateRequest { incremental: false, x: 0, y: 0, width: 320, height: 200 };
    req.encode(&mut buf);
    assert_eq!(buf, client_lines[4]);
    assert_eq!(ClientMessage::decode(&buf, &limits).unwrap(), (req, 10));

    // The server's FramebufferUpdate: header, one Raw rect, then pixels.
    let rest: Vec<u8> = lines.flatten().copied().collect();
    let (msg, n1) = ServerMessage::decode(&rest, &limits).unwrap();
    assert_eq!(msg, ServerMessage::FramebufferUpdate { num_rects: 1 });
    let (rect, n2) = RectHeader::decode(&rest[n1..]).unwrap();
    assert_eq!(rect, RectHeader { x: 0, y: 0, width: 320, height: 200, encoding: Encoding::Raw });
    let payload_seen = rest.len() - n1 - n2 + v.elided;
    assert_eq!(rect.raw_payload_len(&si.pixel_format), payload_seen);
}

#[test]
fn server_handshake_reproduces_x11vnc_session() {
    let v = load();
    let server_lines = side(&v, true);
    let client_lines = side(&v, false);

    // ServerInit as x11vnc sent it (name taken from the transcript).
    let (si, _) = ServerInit::decode(&server_lines[4..9].concat(), &Limits::default()).unwrap();
    let cfg = ServerConfig {
        version: Version::V3_8,
        security: vec![SecurityType::None],
        server_init: si,
        vnc_auth: None,
        vencrypt: None,
        limits: Limits::default(),
    };
    let mut hs = ServerHandshake::new(cfg);
    let mut out = hs.take_output();
    for line in &client_lines[..3] {
        let used = hs.receive(line).expect("handshake");
        assert_eq!(used, line.len());
        out.extend(hs.take_output());
    }
    assert!(hs.is_done());
    assert_eq!(hs.client_init(), Some(ClientInit { shared: true }));

    let mut expected: Vec<u8> = server_lines[..9].concat();
    // x11vnc writes true-colour-flag as 0xff; we canonicalise flags to 0x01.
    // Both are valid (RFC 6143: any nonzero value means true).
    let tc = 12 + 2 + 4 + 4 + 3;
    assert_eq!(expected[tc], 0xff);
    expected[tc] = 0x01;
    assert_eq!(out, expected);
}
