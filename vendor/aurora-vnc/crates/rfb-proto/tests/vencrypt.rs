//! VeNCrypt negotiation, byte-exact, at every chunk size, in both directions.
//! TLS itself is outside rfb-proto, so these tests use an identity "TLS"
//! layer: they check that each side pauses at exactly the right byte, and
//! that the inner authentication that follows is byte-exact. The real-TLS
//! end-to-end tests live in `crates/rfb-tls/tests`.

use rfb_proto::*;

const CHALLENGE: [u8; 16] = *b"0123456789abcdef";

fn server_init() -> ServerInit {
    ServerInit { width: 640, height: 480, pixel_format: PixelFormat::RGBX8888_LE, name: b"aurora".to_vec() }
}

fn server_cfg(version: Version) -> ServerConfig {
    let mut cfg = ServerConfig::new(server_init());
    cfg.version = version;
    cfg.vnc_auth = Some(VncAuthConfig { password: b"vncpass".to_vec(), challenge: CHALLENGE });
    cfg.vencrypt.as_mut().unwrap().plain_auth = true;
    cfg
}

fn client_cfg(version: Version) -> ClientConfig {
    ClientConfig { max_version: version, ..ClientConfig::encrypted() }
}

#[derive(Default, Debug)]
struct Transcript {
    /// Everything the client sent, and the server sent, in order.
    c2s: Vec<u8>,
    s2c: Vec<u8>,
    /// How many bytes each side had sent in the clear when it started TLS.
    c_plain: Option<usize>,
    s_plain: Option<usize>,
}

/// Pumps both state machines, `chunk` bytes at a time, through an identity
/// TLS layer. `verdict` decides Plain credentials on the server side.
fn run(
    mut c: ClientHandshake,
    mut s: ServerHandshake,
    chunk: usize,
    verdict: impl Fn(&PlainCredentials) -> bool,
) -> (Result<ClientHandshake, HandshakeError>, Result<ServerHandshake, HandshakeError>, Transcript) {
    let mut t = Transcript::default();
    let (mut to_c, mut to_s) = (Vec::new(), Vec::new());
    let (mut c_buf, mut s_buf) = (Vec::new(), Vec::new());
    let mut s_err = None;
    for _ in 0..100_000 {
        if c.needs_tls().is_some() {
            let out = c.take_output();
            t.c2s.extend(&out);
            t.c_plain = Some(t.c2s.len());
            to_s.extend(out);
            c.tls_established().unwrap();
        }
        if s_err.is_none() && s.needs_tls().is_some() {
            let out = s.take_output();
            t.s2c.extend(&out);
            t.s_plain = Some(t.s2c.len());
            to_c.extend(out);
            s.tls_established().unwrap();
        }
        if s_err.is_none() {
            if let Some(creds) = s.plain_credentials() {
                let ok = verdict(creds);
                if let Err(e) = s.resolve_plain(ok) {
                    s_err = Some(e);
                }
            }
        }
        let out = s.take_output();
        t.s2c.extend(&out);
        to_c.extend(out);
        let out = c.take_output();
        t.c2s.extend(&out);
        to_s.extend(out);
        if c.is_done() && s.is_done() && to_c.is_empty() && to_s.is_empty() {
            return (Ok(c), Ok(s), t);
        }
        let n = chunk.min(to_c.len());
        c_buf.extend(to_c.drain(..n));
        match c.receive(&c_buf) {
            Ok(u) => drop(c_buf.drain(..u)),
            Err(e) => return (Err(e), s_err.map_or(Ok(s), Err), t),
        }
        if s_err.is_none() {
            let n = chunk.min(to_s.len());
            s_buf.extend(to_s.drain(..n));
            match s.receive(&s_buf) {
                Ok(u) => drop(s_buf.drain(..u)),
                Err(e) => s_err = Some(e),
            }
        }
        if to_c.is_empty() && to_s.is_empty() && c_buf.is_empty() && s_buf.is_empty() && s_err.is_some() {
            // Server failed; deliver whatever it queued, then stop.
            let out = s.take_output();
            t.s2c.extend(&out);
            c_buf.extend(out);
            let r = c.receive(&c_buf);
            return (r.map(|_| c), Err(s_err.unwrap()), t);
        }
    }
    panic!("handshake did not converge");
}

const CHUNKS: [usize; 8] = [1, 2, 3, 4, 5, 7, 13, 4096];

fn be(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// The unencrypted prefix both sides send, up to and including the TLS ack.
fn plain_prefix(version: Version, chosen: u32) -> (Vec<u8>, Vec<u8>) {
    let mut s2c = version.wire().to_vec();
    s2c.extend([1, 19]); // one security type: VeNCrypt
    s2c.extend([0, 2]); // VeNCrypt 0.2
    s2c.extend([0, 2]); // version ack, two subtypes
    s2c.extend(be(261)); // X509Vnc
    s2c.extend(be(262)); // X509Plain
    s2c.push(1); // TLS ack
    let mut c2s = version.wire().to_vec();
    c2s.push(19);
    c2s.extend([0, 2]);
    c2s.extend(be(chosen));
    (c2s, s2c)
}

fn server_init_bytes() -> Vec<u8> {
    let mut v = Vec::new();
    server_init().encode(&mut v);
    v
}

#[test]
fn x509plain_is_byte_exact_at_every_chunk_size() {
    for version in [Version::V3_7, Version::V3_8] {
        for chunk in CHUNKS {
            let mut cfg = client_cfg(version);
            cfg.username = Some(b"user".to_vec());
            cfg.password = Some(b"secret".to_vec());
            let (c, s, t) = run(ClientHandshake::new(cfg), ServerHandshake::new(server_cfg(version)), chunk, |cr| {
                cr.matches(b"user", b"secret")
            });
            let (c, s) = (c.unwrap(), s.unwrap());
            let (mut c2s, mut s2c) = plain_prefix(version, 262);
            assert_eq!(t.c_plain, Some(c2s.len()), "{version:?} chunk {chunk}");
            assert_eq!(t.s_plain, Some(s2c.len()), "{version:?} chunk {chunk}");
            c2s.extend(be(4));
            c2s.extend(be(6));
            c2s.extend(b"usersecret");
            c2s.push(1); // ClientInit shared
            s2c.extend(be(0)); // SecurityResult OK
            s2c.extend(server_init_bytes());
            assert_eq!(t.c2s, c2s, "{version:?} chunk {chunk}");
            assert_eq!(t.s2c, s2c, "{version:?} chunk {chunk}");
            assert_eq!(c.security_type(), Some(SecurityType::VeNCrypt));
            assert_eq!(c.vencrypt_subtype(), Some(VeNCryptSubtype::X509Plain));
            assert_eq!(s.vencrypt_subtype(), Some(VeNCryptSubtype::X509Plain));
            assert_eq!(c.server_init(), Some(&server_init()));
        }
    }
}

#[test]
fn x509vnc_is_byte_exact_at_every_chunk_size() {
    for version in [Version::V3_7, Version::V3_8] {
        for chunk in CHUNKS {
            let mut cfg = client_cfg(version);
            cfg.password = Some(b"vncpass".to_vec());
            let (c, s, t) = run(ClientHandshake::new(cfg), ServerHandshake::new(server_cfg(version)), chunk, |_| false);
            let (c, s) = (c.unwrap(), s.unwrap());
            let (mut c2s, mut s2c) = plain_prefix(version, 261);
            assert_eq!(t.c_plain, Some(c2s.len()));
            assert_eq!(t.s_plain, Some(s2c.len()));
            c2s.extend(auth::vnc_auth_response(b"vncpass", &CHALLENGE));
            c2s.push(1);
            s2c.extend(CHALLENGE);
            s2c.extend(be(0));
            s2c.extend(server_init_bytes());
            assert_eq!(t.c2s, c2s, "{version:?} chunk {chunk}");
            assert_eq!(t.s2c, s2c, "{version:?} chunk {chunk}");
            assert_eq!(s.vencrypt_subtype(), Some(VeNCryptSubtype::X509Vnc));
            assert!(c.is_done());
        }
    }
}

#[test]
fn x509none_sends_security_result_even_on_3_7() {
    for version in [Version::V3_7, Version::V3_8] {
        for chunk in CHUNKS {
            let mut scfg = server_cfg(version);
            scfg.vencrypt.as_mut().unwrap().subtypes = vec![VeNCryptSubtype::X509None];
            let (c, _, t) =
                run(ClientHandshake::new(client_cfg(version)), ServerHandshake::new(scfg), chunk, |_| false);
            c.unwrap();
            let mut s2c = version.wire().to_vec();
            s2c.extend([1, 19, 0, 2, 0, 1]);
            s2c.extend(be(260));
            s2c.push(1);
            s2c.extend(be(0));
            s2c.extend(server_init_bytes());
            assert_eq!(t.s2c, s2c);
        }
    }
}

#[test]
fn server_picks_vencrypt_for_rfb_3_3() {
    for chunk in CHUNKS {
        let mut cfg = client_cfg(Version::V3_3);
        cfg.password = Some(b"vncpass".to_vec());
        let (c, _, t) =
            run(ClientHandshake::new(cfg), ServerHandshake::new(server_cfg(Version::V3_8)), chunk, |_| false);
        let c = c.unwrap();
        assert_eq!(c.version(), Some(Version::V3_3));
        assert_eq!(&t.s2c[12..16], &be(19));
        assert_eq!(c.vencrypt_subtype(), Some(VeNCryptSubtype::X509Vnc));
    }
}

#[test]
fn wrong_plain_password_fails_with_reason() {
    for chunk in CHUNKS {
        let mut cfg = client_cfg(Version::V3_8);
        cfg.username = Some(b"user".to_vec());
        cfg.password = Some(b"wrong".to_vec());
        let (c, s, _) = run(ClientHandshake::new(cfg), ServerHandshake::new(server_cfg(Version::V3_8)), chunk, |cr| {
            cr.matches(b"user", b"secret")
        });
        assert_eq!(c.err(), Some(HandshakeError::AuthFailed(Some("authentication failed".into()))));
        assert_eq!(s.err(), Some(HandshakeError::AuthFailed(None)));
    }
}

#[test]
fn wrong_vnc_password_inside_tls_fails() {
    let mut cfg = client_cfg(Version::V3_7);
    cfg.password = Some(b"nope".to_vec());
    let (c, s, _) = run(ClientHandshake::new(cfg), ServerHandshake::new(server_cfg(Version::V3_7)), 4096, |_| false);
    assert_eq!(c.err(), Some(HandshakeError::AuthFailed(None)));
    assert_eq!(s.err(), Some(HandshakeError::AuthFailed(None)));
}

#[test]
fn no_common_subtype_is_reported() {
    // The server only offers Plain; the client has no username.
    let mut scfg = server_cfg(Version::V3_8);
    scfg.vencrypt.as_mut().unwrap().subtypes = vec![VeNCryptSubtype::X509Plain];
    let (c, _, _) = run(ClientHandshake::new(client_cfg(Version::V3_8)), ServerHandshake::new(scfg), 1, |_| true);
    assert_eq!(c.err(), Some(HandshakeError::NoCommonSubtype { offered: vec![VeNCryptSubtype::X509Plain] }));
}

/// Feeds a hand-built server transcript to a fresh client.
fn client_after(server_bytes: &[u8]) -> (ClientHandshake, Result<usize, HandshakeError>) {
    let mut cfg = client_cfg(Version::V3_8);
    cfg.password = Some(b"pw".to_vec());
    let mut c = ClientHandshake::new(cfg);
    let mut input = b"RFB 003.008\n".to_vec();
    input.extend([1, 19]);
    input.extend(server_bytes);
    let r = c.receive(&input);
    (c, r)
}

#[test]
fn client_refuses_old_vencrypt_version() {
    let (_, r) = client_after(&[0, 1]);
    assert_eq!(r, Err(HandshakeError::VeNCryptVersion { major: 0, minor: 1 }));
}

#[test]
fn client_answers_newer_vencrypt_version_with_0_2() {
    let (mut c, r) = client_after(&[0, 3]);
    assert!(r.is_ok());
    let out = c.take_output();
    assert_eq!(&out[out.len() - 2..], &[0, 2]);
}

#[test]
fn client_reports_server_version_rejection() {
    let (_, r) = client_after(&[0, 2, 0xff]);
    assert!(matches!(r, Err(HandshakeError::Refused(_))));
}

#[test]
fn client_rejects_oversized_subtype_list_before_reading_it() {
    let (_, r) = client_after(&[0, 2, 0, 200]);
    assert!(matches!(r, Err(HandshakeError::Decode(DecodeError::Invalid(_)))));
}

#[test]
fn client_rejects_empty_subtype_list() {
    let (_, r) = client_after(&[0, 2, 0, 0]);
    assert_eq!(r, Err(HandshakeError::NoCommonSubtype { offered: vec![] }));
}

#[test]
fn client_reports_tls_refusal() {
    let mut b = vec![0, 2, 0, 1];
    b.extend(be(261));
    b.push(0);
    let (_, r) = client_after(&b);
    assert_eq!(r, Err(HandshakeError::TlsRefused));
}

#[test]
fn client_consumes_nothing_while_waiting_for_tls() {
    let mut b = vec![0, 2, 0, 1];
    b.extend(be(261));
    b.push(1);
    b.extend([0x16, 0x03, 0x03]); // looks like the start of a TLS record
    let (mut c, r) = client_after(&b);
    let total = 12 + 2 + b.len();
    assert_eq!(r, Ok(total - 3), "TLS bytes must be left for the caller");
    assert_eq!(c.needs_tls(), Some(VeNCryptSubtype::X509Vnc));
    assert!(!c.is_done());
    assert_eq!(c.receive(&[0x16]), Ok(0));
    c.tls_established().unwrap();
    assert_eq!(c.needs_tls(), None);
    assert_eq!(
        c.tls_established(),
        Err(HandshakeError::InvalidState("tls_established called when TLS was not requested"))
    );
}

/// Drives a server up to the point where it waits for the client's subtype.
fn server_at_subtype_choice() -> ServerHandshake {
    let mut s = ServerHandshake::new(server_cfg(Version::V3_8));
    s.receive(b"RFB 003.008\n").unwrap();
    s.receive(&[19]).unwrap();
    s.receive(&[0, 2]).unwrap();
    s.take_output();
    s
}

#[test]
fn server_refuses_unsupported_vencrypt_version() {
    let mut s = ServerHandshake::new(server_cfg(Version::V3_8));
    s.receive(b"RFB 003.008\n").unwrap();
    s.receive(&[19]).unwrap();
    s.take_output();
    assert_eq!(s.receive(&[0, 1]), Err(HandshakeError::VeNCryptVersion { major: 0, minor: 1 }));
    assert_eq!(s.take_output(), vec![0xff]);
}

#[test]
fn server_refuses_subtype_it_did_not_offer() {
    for code in [256u32, 260, 257, 2, 0xdead_beef] {
        let mut s = server_at_subtype_choice();
        let r = s.receive(&be(code));
        assert_eq!(r, Err(HandshakeError::UnexpectedSubtype(VeNCryptSubtype::from_code(code))));
        assert_eq!(s.take_output(), vec![0], "TLS ack must be 'refused'");
        assert_eq!(s.needs_tls(), None);
    }
}

#[test]
fn server_never_starts_inner_auth_before_tls() {
    let mut s = server_at_subtype_choice();
    let mut input = be(261).to_vec();
    input.extend([0x16, 0x03, 0x01, 0x02]); // a ClientHello arriving early
    assert_eq!(s.receive(&input), Ok(4));
    assert_eq!(s.take_output(), vec![1]);
    assert_eq!(s.needs_tls(), Some(VeNCryptSubtype::X509Vnc));
    assert_eq!(s.receive(&input[4..]), Ok(0));
    assert!(s.take_output().is_empty(), "no challenge may leave before TLS is up");
    s.tls_established().unwrap();
    assert_eq!(s.take_output(), CHALLENGE.to_vec());
}

#[test]
fn server_rejects_oversized_plain_credentials() {
    let mut s = server_at_subtype_choice();
    s.receive(&be(262)).unwrap();
    s.tls_established().unwrap();
    let mut input = be(5).to_vec();
    input.extend(be(1_000_000));
    assert!(matches!(s.receive(&input), Err(HandshakeError::Decode(DecodeError::Invalid(_)))));
    assert!(s.plain_credentials().is_none());
}

#[test]
fn resolve_plain_needs_pending_credentials() {
    let mut s = server_at_subtype_choice();
    assert!(matches!(s.resolve_plain(true), Err(HandshakeError::InvalidState(_))));
}

#[test]
fn anonymous_tls_is_off_by_default_on_both_sides() {
    // Server: anonymous subtypes configured but not allowed, so VeNCrypt has
    // nothing to offer and the connection is refused.
    let mut scfg = server_cfg(Version::V3_8);
    scfg.vencrypt.as_mut().unwrap().subtypes = vec![VeNCryptSubtype::TlsVnc, VeNCryptSubtype::TlsNone];
    let (c, _, _) =
        run(ClientHandshake::new(client_cfg(Version::V3_8)), ServerHandshake::new(scfg.clone()), 4096, |_| false);
    assert_eq!(c.err(), Some(HandshakeError::Refused("no security type configured".into())));

    // Client: server offers anonymous only, client does not allow it.
    scfg.vencrypt.as_mut().unwrap().allow_anonymous_tls = true;
    let mut ccfg = client_cfg(Version::V3_8);
    ccfg.vencrypt_subtypes.push(VeNCryptSubtype::TlsNone);
    let (c, _, _) = run(ClientHandshake::new(ccfg.clone()), ServerHandshake::new(scfg.clone()), 4096, |_| false);
    assert_eq!(
        c.err(),
        Some(HandshakeError::NoCommonSubtype { offered: vec![VeNCryptSubtype::TlsVnc, VeNCryptSubtype::TlsNone] })
    );

    // Both opt in: works, and the caller is told it is anonymous.
    ccfg.allow_anonymous_tls = true;
    let (c, _, _) = run(ClientHandshake::new(ccfg), ServerHandshake::new(scfg), 4096, |_| false);
    let c = c.unwrap();
    assert_eq!(c.vencrypt_subtype(), Some(VeNCryptSubtype::TlsNone));
    assert!(VeNCryptSubtype::TlsNone.is_anonymous());
}

#[test]
fn secure_server_defaults_offer_nothing_without_credentials() {
    let s = ServerConfig::new(server_init());
    assert_eq!(s.security, vec![SecurityType::VeNCrypt]);
    let (c, _, _) = run(ClientHandshake::new(ClientConfig::encrypted()), ServerHandshake::new(s), 4096, |_| false);
    assert_eq!(c.err(), Some(HandshakeError::Refused("no security type configured".into())));
}

#[test]
fn secure_server_defaults_offer_x509_vnc_first() {
    let mut s = ServerConfig::new(server_init());
    s.vnc_auth = Some(VncAuthConfig { password: b"pw".to_vec(), challenge: CHALLENGE });
    let mut h = ServerHandshake::new(s);
    h.receive(b"RFB 003.008\n").unwrap();
    h.receive(&[19]).unwrap();
    h.receive(&[0, 2]).unwrap();
    let out = h.take_output();
    let mut expected = b"RFB 003.008\n".to_vec();
    expected.extend([1, 19, 0, 2, 0, 1]);
    expected.extend(be(261));
    assert_eq!(out, expected, "Plain is not offered until the embedder can verify it");
}

#[test]
fn plain_credentials_are_redacted() {
    let c = PlainCredentials { username: b"alice".to_vec(), password: b"hunter2".to_vec() };
    let d = format!("{c:?}");
    assert!(d.contains("alice") && !d.contains("hunter2"));
    let cfg = ClientConfig { password: Some(b"hunter2".to_vec()), ..ClientConfig::encrypted() };
    assert!(!format!("{cfg:?}").contains("hunter2"));
}

#[test]
fn subtype_codes_round_trip() {
    for code in 250..270u32 {
        assert_eq!(VeNCryptSubtype::from_code(code).code(), code);
    }
    assert!(!VeNCryptSubtype::Plain.uses_tls());
    assert!(VeNCryptSubtype::X509Plain.uses_tls() && !VeNCryptSubtype::X509Plain.is_anonymous());
}
