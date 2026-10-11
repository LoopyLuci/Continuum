//! Blocking `connect` / `accept` over real loopback TCP, including the first
//! messages after the handshake, so the upgraded `Stream` is proven usable.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use rfb_proto::*;
use rfb_tls::rustls::ProtocolVersion;
use rfb_tls::{accept, client_config, connect, server_config, ClientTls, SelfSigned, ServerVerification};

fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    (l, addr)
}

fn timeouts(s: &TcpStream) {
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
}

fn server_init() -> ServerInit {
    ServerInit { width: 64, height: 48, pixel_format: PixelFormat::RGBX8888_LE, name: b"tcp-test".to_vec() }
}

#[test]
fn vencrypt_plain_over_tcp_then_messages_flow() {
    let cert = SelfSigned::generate(&["localhost"]).unwrap();
    let scfg = server_config(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap();
    let (l, addr) = listener();
    let server = thread::spawn(move || {
        let (sock, _) = l.accept().unwrap();
        timeouts(&sock);
        let mut cfg = ServerConfig::new(server_init());
        cfg.vencrypt.as_mut().unwrap().plain_auth = true;
        let mut a = accept(sock, ServerHandshake::new(cfg), Some(scfg), |c| c.matches(b"bob", b"pa55word")).unwrap();
        assert_eq!(a.stream.tls_info().unwrap().protocol, ProtocolVersion::TLSv1_3);
        // Read one client message (FramebufferUpdateRequest) through TLS.
        let mut buf = a.leftover.clone();
        let mut chunk = [0u8; 64];
        loop {
            match ClientMessage::decode(&buf, &Limits::default()) {
                Ok((msg, _)) => break msg,
                Err(DecodeError::Incomplete { .. }) => {
                    let n = a.stream.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) => panic!("{e}"),
            }
        }
    });

    let sock = TcpStream::connect(&addr).unwrap();
    timeouts(&sock);
    let ccfg = ClientConfig {
        username: Some(b"bob".to_vec()),
        password: Some(b"pa55word".to_vec()),
        ..ClientConfig::encrypted()
    };
    let tls = ClientTls {
        config: client_config(ServerVerification::PinnedSha256(cert.sha256)).unwrap(),
        server_name: "localhost".into(),
    };
    let mut c = connect(sock, ClientHandshake::new(ccfg), Some(&tls)).unwrap();
    assert!(c.stream.is_tls());
    assert_eq!(c.handshake.server_init(), Some(&server_init()));
    assert_eq!(c.stream.tls_info().unwrap().peer_certificate_sha256, Some(cert.sha256));
    let req = ClientMessage::FramebufferUpdateRequest { incremental: false, x: 0, y: 0, width: 64, height: 48 };
    let mut out = Vec::new();
    req.encode(&mut out);
    c.stream.write_all(&out).unwrap();
    c.stream.flush().unwrap();
    assert_eq!(server.join().unwrap(), req);
}

#[test]
fn plain_rfb_still_works_without_tls_when_explicitly_configured() {
    let (l, addr) = listener();
    let server = thread::spawn(move || {
        let (sock, _) = l.accept().unwrap();
        timeouts(&sock);
        let mut cfg = ServerConfig::new(server_init());
        cfg.security = vec![SecurityType::None];
        cfg.vencrypt = None;
        let a = accept(sock, ServerHandshake::new(cfg), None, |_| false).unwrap();
        assert!(!a.stream.is_tls());
    });
    let sock = TcpStream::connect(&addr).unwrap();
    timeouts(&sock);
    let c = connect(sock, ClientHandshake::new(ClientConfig::default()), None).unwrap();
    assert!(!c.stream.is_tls());
    assert_eq!(c.handshake.security_type(), Some(SecurityType::None));
    server.join().unwrap();
}

#[test]
fn bad_plain_login_over_tcp_reports_auth_failure_to_both_sides() {
    let cert = SelfSigned::generate(&["localhost"]).unwrap();
    let scfg = server_config(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap();
    let (l, addr) = listener();
    let server = thread::spawn(move || {
        let (sock, _) = l.accept().unwrap();
        timeouts(&sock);
        let mut cfg = ServerConfig::new(server_init());
        cfg.vencrypt.as_mut().unwrap().plain_auth = true;
        accept(sock, ServerHandshake::new(cfg), Some(scfg), |_| false).err().unwrap().to_string()
    });
    let sock = TcpStream::connect(&addr).unwrap();
    timeouts(&sock);
    let ccfg =
        ClientConfig { username: Some(b"bob".to_vec()), password: Some(b"nope".to_vec()), ..ClientConfig::encrypted() };
    let tls = ClientTls {
        config: client_config(ServerVerification::PinnedSha256(cert.sha256)).unwrap(),
        server_name: "localhost".into(),
    };
    let err = connect(sock, ClientHandshake::new(ccfg), Some(&tls)).err().unwrap().to_string();
    assert!(err.contains("authentication failed"), "{err}");
    assert!(server.join().unwrap().contains("authentication failed"));
}

#[test]
fn self_signed_cert_persists_across_runs() {
    let dir = std::env::temp_dir().join(format!("aurora-tls-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let first = SelfSigned::load_or_generate(&dir, &["localhost"]).unwrap();
    let second = SelfSigned::load_or_generate(&dir, &["localhost"]).unwrap();
    assert_eq!(first.sha256, second.sha256);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("key.pem")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "private key must not be group/world readable");
    }
    assert!(!format!("{first:?}").contains("PRIVATE KEY"));
    let fp = rfb_tls::format_fingerprint(&first.sha256);
    assert_eq!(rfb_tls::parse_fingerprint(&fp).unwrap(), first.sha256);
    assert!(rfb_tls::parse_fingerprint("zz").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
