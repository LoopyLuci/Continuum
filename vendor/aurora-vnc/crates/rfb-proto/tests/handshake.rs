use rfb_proto::*;

fn server_init() -> ServerInit {
    ServerInit { width: 1920, height: 1080, pixel_format: PixelFormat::RGBX8888_LE, name: b"aurora".to_vec() }
}

fn server_cfg(version: Version, security: Vec<SecurityType>) -> ServerConfig {
    ServerConfig {
        version,
        security,
        server_init: server_init(),
        vnc_auth: None,
        vencrypt: None,
        limits: Limits::default(),
    }
}

/// Pump bytes between the two state machines, delivering `chunk` bytes at a time.
fn run(
    mut c: ClientHandshake,
    mut s: ServerHandshake,
    chunk: usize,
) -> (Result<ClientHandshake, HandshakeError>, ServerHandshake) {
    let (mut to_c, mut to_s) = (Vec::new(), Vec::new());
    let (mut c_buf, mut s_buf) = (Vec::new(), Vec::new());
    for _ in 0..10_000 {
        to_c.extend(s.take_output());
        to_s.extend(c.take_output());
        if c.is_done() && s.is_done() && to_c.is_empty() && to_s.is_empty() {
            return (Ok(c), s);
        }
        if to_c.is_empty() && to_s.is_empty() {
            break;
        }
        let n = chunk.min(to_c.len());
        c_buf.extend(to_c.drain(..n));
        match c.receive(&c_buf) {
            Ok(u) => drop(c_buf.drain(..u)),
            Err(e) => return (Err(e), s),
        }
        let n = chunk.min(to_s.len());
        s_buf.extend(to_s.drain(..n));
        if let Ok(u) = s.receive(&s_buf) {
            s_buf.drain(..u);
        }
    }
    (Err(HandshakeError::Finished), s)
}

#[test]
fn every_version_pair_completes_at_every_chunk_size() {
    let vs = [Version::V3_3, Version::V3_7, Version::V3_8];
    for &cv in &vs {
        for &sv in &vs {
            for chunk in [1, 2, 3, 7, 4096] {
                let c = ClientHandshake::new(ClientConfig { max_version: cv, ..Default::default() });
                let s = ServerHandshake::new(server_cfg(sv, vec![SecurityType::None]));
                let (c, s) = run(c, s, chunk);
                let c = c.unwrap_or_else(|e| panic!("client {cv:?} server {sv:?} chunk {chunk}: {e}"));
                assert_eq!(c.version(), Some(cv.min(sv)));
                assert_eq!(s.version(), Some(cv.min(sv)));
                assert_eq!(c.server_init(), Some(&server_init()));
                assert_eq!(s.client_init(), Some(ClientInit { shared: true }));
            }
        }
    }
}

#[test]
fn no_common_security_is_reported() {
    let c = ClientHandshake::new(ClientConfig::default());
    let mut cfg = server_cfg(Version::V3_8, vec![SecurityType::VncAuth]);
    cfg.vnc_auth = Some(VncAuthConfig { password: b"pw".to_vec(), challenge: [1; 16] });
    let (c, _) = run(c, ServerHandshake::new(cfg), 4096);
    assert!(matches!(c, Err(HandshakeError::NoCommonSecurity { .. })));
}

#[test]
fn server_with_nothing_implementable_refuses_with_reason() {
    // VeNCrypt is listed but has no TLS settings: refuse, never half-offer.
    let c = ClientHandshake::new(ClientConfig::default());
    let s = ServerHandshake::new(server_cfg(Version::V3_8, vec![SecurityType::VeNCrypt]));
    let (c, _) = run(c, s, 4096);
    assert_eq!(c.err(), Some(HandshakeError::Refused("no security type configured".into())));
}

#[test]
fn server_refusal_reason_reaches_client() {
    let mut c = ClientHandshake::new(ClientConfig::default());
    let mut input = b"RFB 003.008\n".to_vec();
    input.push(0);
    input.extend_from_slice(&(4u32).to_be_bytes());
    input.extend_from_slice(b"busy");
    assert_eq!(c.receive(&input), Err(HandshakeError::Refused("busy".into())));
    assert_eq!(c.receive(&[]), Err(HandshakeError::Finished));
}

#[test]
fn hostile_name_length_is_rejected_before_allocating() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0, 1, 0, 1]);
    PixelFormat::RGBX8888_LE.encode(&mut buf);
    buf.extend_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(ServerInit::decode(&buf, &Limits::default()), Err(DecodeError::Invalid(_))));
}

#[test]
fn odd_versions_map_per_rfc() {
    assert_eq!(Version::decode(b"RFB 003.889\n").unwrap().0, Version::V3_8); // Apple
    assert_eq!(Version::decode(b"RFB 003.008\n").unwrap().0, Version::V3_8);
    assert_eq!(Version::decode(b"RFB 003.009\n").unwrap().0, Version::V3_8);
    assert_eq!(Version::decode(b"RFB 004.001\n").unwrap().0, Version::V3_8);
    assert!(Version::V3_8.uses_38_security());
    assert!(!Version::V3_7.uses_38_security());
    assert_eq!(Version::decode(b"RFB 003.005\n").unwrap().0, Version::V3_3);
    assert!(Version::decode(b"HTTP/1.1 200").is_err());
    assert_eq!(Version::decode(b"RFB 003"), Err(DecodeError::Incomplete { needed: 12 }));
}

fn vnc_server(version: Version, password: &[u8]) -> ServerHandshake {
    let mut cfg = server_cfg(version, vec![SecurityType::VncAuth, SecurityType::None]);
    cfg.vnc_auth = Some(VncAuthConfig { password: password.to_vec(), challenge: *b"0123456789abcdef" });
    ServerHandshake::new(cfg)
}

fn vnc_client(version: Version, password: Option<&[u8]>) -> ClientHandshake {
    ClientHandshake::new(ClientConfig {
        max_version: version,
        security: vec![SecurityType::VncAuth],
        password: password.map(<[u8]>::to_vec),
        ..Default::default()
    })
}

#[test]
fn vnc_auth_succeeds_at_every_version_and_chunk_size() {
    for v in [Version::V3_3, Version::V3_7, Version::V3_8] {
        for chunk in [1, 5, 4096] {
            let (c, s) = run(vnc_client(v, Some(b"s3cret")), vnc_server(v, b"s3cret"), chunk);
            let c = c.unwrap_or_else(|e| panic!("{v:?} chunk {chunk}: {e}"));
            assert_eq!(c.security_type(), Some(SecurityType::VncAuth));
            assert_eq!(s.security_type(), Some(SecurityType::VncAuth));
            assert!(s.is_done());
        }
    }
}

#[test]
fn vnc_auth_wrong_password_fails_with_reason_on_3_8() {
    let (c, s) = run(vnc_client(Version::V3_8, Some(b"guess")), vnc_server(Version::V3_8, b"s3cret"), 4096);
    assert_eq!(c.err(), Some(HandshakeError::AuthFailed(Some("authentication failed".into()))));
    assert!(!s.is_done());
    let (c, _) = run(vnc_client(Version::V3_7, Some(b"guess")), vnc_server(Version::V3_7, b"s3cret"), 4096);
    assert_eq!(c.err(), Some(HandshakeError::AuthFailed(None)));
}

#[test]
fn vnc_auth_only_uses_first_8_password_bytes() {
    // Historical VNC behaviour, kept for compatibility and tested so it is
    // never "fixed" by accident.
    let (c, _) = run(vnc_client(Version::V3_8, Some(b"12345678-extra")), vnc_server(Version::V3_8, b"12345678"), 4096);
    assert!(c.is_ok());
}

#[test]
fn client_without_password_never_picks_vnc_auth() {
    let mut cfg = server_cfg(Version::V3_8, vec![SecurityType::VncAuth]);
    cfg.vnc_auth = Some(VncAuthConfig { password: b"x".to_vec(), challenge: [7; 16] });
    let c = ClientHandshake::new(ClientConfig {
        security: vec![SecurityType::VncAuth, SecurityType::None],
        ..Default::default()
    });
    let (c, _) = run(c, ServerHandshake::new(cfg), 4096);
    assert!(matches!(c, Err(HandshakeError::NoCommonSecurity { .. })));
}

#[test]
fn server_never_offers_vnc_auth_without_a_password() {
    let mut s = ServerHandshake::new(server_cfg(Version::V3_8, vec![SecurityType::VncAuth, SecurityType::None]));
    s.take_output();
    s.receive(b"RFB 003.008\n").unwrap();
    assert_eq!(s.take_output(), vec![1, 1]); // one type offered: None
}

#[test]
fn passwords_are_redacted_in_debug_output() {
    let c = ClientConfig { password: Some(b"hunter2".to_vec()), ..Default::default() };
    assert!(!format!("{c:?}").contains("hunter2"));
    let a = VncAuthConfig { password: b"hunter2".to_vec(), challenge: [0; 16] };
    assert!(!format!("{a:?}").contains("hunter2"));
}
