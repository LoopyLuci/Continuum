use rfb_proto::*;

fn client_samples() -> Vec<ClientMessage> {
    vec![
        ClientMessage::SetPixelFormat(PixelFormat::RGBX8888_LE),
        ClientMessage::SetEncodings(vec![
            Encoding::Zrle,
            Encoding::Raw,
            Encoding::DesktopSize,
            Encoding::Other(-1_000_000),
        ]),
        ClientMessage::FramebufferUpdateRequest { incremental: true, x: 1, y: 2, width: 3, height: 4 },
        ClientMessage::KeyEvent { down: true, keysym: 0xff0d },
        ClientMessage::PointerEvent { button_mask: 0b101, x: 640, y: 480 },
        ClientMessage::ClientCutText(b"hello".to_vec()),
    ]
}

fn server_samples() -> Vec<ServerMessage> {
    vec![
        ServerMessage::FramebufferUpdate { num_rects: 3 },
        ServerMessage::SetColourMapEntries { first_colour: 7, colours: vec![[1, 2, 3], [65535, 0, 9]] },
        ServerMessage::Bell,
        ServerMessage::ServerCutText("héllo".as_bytes().to_vec()),
    ]
}

#[test]
fn round_trip_and_every_prefix_is_incomplete() {
    let limits = Limits::default();
    for m in client_samples() {
        let mut b = Vec::new();
        m.encode(&mut b);
        assert_eq!(ClientMessage::decode(&b, &limits).unwrap(), (m.clone(), b.len()));
        for cut in 0..b.len() {
            assert!(
                matches!(ClientMessage::decode(&b[..cut], &limits), Err(DecodeError::Incomplete { .. })),
                "{m:?} cut {cut}"
            );
        }
    }
    for m in server_samples() {
        let mut b = Vec::new();
        m.encode(&mut b);
        assert_eq!(ServerMessage::decode(&b, &limits).unwrap(), (m.clone(), b.len()));
        for cut in 0..b.len() {
            assert!(
                matches!(ServerMessage::decode(&b[..cut], &limits), Err(DecodeError::Incomplete { .. })),
                "{m:?} cut {cut}"
            );
        }
    }
}

#[test]
fn decoders_never_panic_on_garbage() {
    // Cheap deterministic fuzz until cargo-fuzz targets land.
    let limits = Limits::default();
    let mut x: u64 = 0x9e3779b97f4a7c15;
    for _ in 0..200_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = (x % 40) as usize;
        let bytes: Vec<u8> = (0..len).map(|i| (x >> (i % 8 * 8)) as u8 ^ i as u8).collect();
        let _ = ClientMessage::decode(&bytes, &limits);
        let _ = ServerMessage::decode(&bytes, &limits);
        let _ = ServerInit::decode(&bytes, &limits);
        let _ = Version::decode(&bytes);
        let _ = RectHeader::decode(&bytes);
    }
}
