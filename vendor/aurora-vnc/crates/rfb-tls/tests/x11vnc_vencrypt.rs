//! Interop: VeNCrypt + TLS 1.3 against a real x11vnc (`-ssl`, OpenSSL).
//! Skips cleanly when Xvfb, x11vnc, or the openssl CLI is missing.
//!
//! x11vnc waits about 1.4 s to tell VeNCrypt clients from "SSL first"
//! clients, so each handshake here takes a couple of seconds.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use rfb_proto::*;
use rfb_tls::rustls::ProtocolVersion;
use rfb_tls::{client_config, connect, fingerprint_sha256, ClientTls, ServerVerification};

const W: u16 = 320;
const H: u16 = 200;

fn have(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Fixture {
    procs: Vec<Child>,
    dir: PathBuf,
    port: u16,
    cert_sha256: [u8; 32],
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for p in self.procs.iter_mut().rev() {
            let _ = p.kill();
            let _ = p.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn der_of(pem_path: &Path) -> Vec<u8> {
    let out =
        Command::new("openssl").args(["x509", "-outform", "DER", "-in"]).arg(pem_path).output().expect("openssl x509");
    assert!(out.status.success());
    out.stdout
}

/// Xvfb + `x11vnc -ssl` with a fresh RSA certificate (x11vnc's OpenSSL setup
/// insists on RSA). `password`: `None` for X509None, `Some` for X509Vnc.
fn x11vnc_ssl(password: Option<&str>) -> Option<Fixture> {
    for bin in ["Xvfb", "x11vnc", "openssl"] {
        if !have(bin) {
            skip(format_args!("{bin} not installed"));
            return None;
        }
    }
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "aurora-x11vnc-ssl-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let ok = Command::new("openssl")
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=localhost"])
        .args(["-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1", "-keyout"])
        .arg(dir.join("key.pem"))
        .arg("-out")
        .arg(dir.join("cert.pem"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        skip(format_args!("openssl could not create a certificate"));
        return None;
    }
    let mut pem = std::fs::read(dir.join("key.pem")).expect("read key.pem");
    pem.extend(std::fs::read(dir.join("cert.pem")).expect("read cert.pem"));
    std::fs::write(dir.join("server.pem"), pem).expect("write server.pem");
    let cert_sha256 = fingerprint_sha256(&der_of(&dir.join("cert.pem")));

    let port = TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).expect("free port").port();
    let fdfile = dir.join("displayfd");
    let xvfb = Command::new("sh")
        .arg("-c")
        .arg(format!("exec Xvfb -displayfd 3 -screen 0 {W}x{H}x24 -nolisten tcp 3>'{}'", fdfile.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn Xvfb");
    let mut fx = Fixture { procs: vec![xvfb], dir: dir.clone(), port, cert_sha256 };
    let deadline = Instant::now() + Duration::from_secs(15);
    let display: u32 = loop {
        if let Ok(s) = std::fs::read_to_string(&fdfile) {
            if s.ends_with('\n') {
                break s.trim().parse().expect("Xvfb -displayfd wrote a display number");
            }
        }
        if Instant::now() > deadline {
            skip(format_args!("Xvfb did not report a display"));
            return None;
        }
        sleep(Duration::from_millis(20));
    };
    let mut cmd = Command::new("x11vnc");
    cmd.args(["-display", &format!(":{display}"), "-rfbport", &port.to_string()])
        .args(["-localhost", "-forever", "-shared", "-quiet", "-noxdamage", "-noshm"])
        .args(["-vencrypt", "nodh:only", "-anontls", "never", "-ssl"])
        .arg(dir.join("server.pem"));
    match password {
        Some(p) => cmd.args(["-passwd", p]),
        None => cmd.arg("-nopw"),
    };
    fx.procs.push(cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("spawn x11vnc"));
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            skip(format_args!("x11vnc -ssl did not start (built without OpenSSL?)"));
            return None;
        }
        sleep(Duration::from_millis(50));
    }
    Some(fx)
}

fn sock(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(20))).unwrap();
    s
}

fn tls(fx: &Fixture) -> ClientTls {
    ClientTls {
        config: client_config(ServerVerification::PinnedSha256(fx.cert_sha256)).unwrap(),
        server_name: "localhost".into(),
    }
}

#[test]
fn x509vnc_with_x11vnc_over_tls13_then_framebuffer_update() {
    let Some(fx) = x11vnc_ssl(Some("s3cret")) else { return };
    let cfg = ClientConfig { password: Some(b"s3cret".to_vec()), ..ClientConfig::encrypted() };
    let mut c = connect(sock(fx.port), ClientHandshake::new(cfg), Some(&tls(&fx))).unwrap();
    assert_eq!(c.handshake.vencrypt_subtype(), Some(VeNCryptSubtype::X509Vnc));
    let info = c.stream.tls_info().unwrap();
    assert_eq!(info.protocol, ProtocolVersion::TLSv1_3);
    assert_eq!(info.peer_certificate_sha256, Some(fx.cert_sha256));
    let si = c.handshake.server_init().unwrap().clone();
    assert_eq!((si.width, si.height), (W, H));

    // The upgraded stream carries the normal message phase.
    let mut out = Vec::new();
    ClientMessage::SetEncodings(vec![Encoding::Raw]).encode(&mut out);
    ClientMessage::FramebufferUpdateRequest { incremental: false, x: 0, y: 0, width: W, height: H }.encode(&mut out);
    c.stream.write_all(&out).unwrap();
    c.stream.flush().unwrap();
    let mut buf = c.leftover;
    let mut chunk = [0u8; 4096];
    while buf.len() < 4 {
        let n = c.stream.read(&mut chunk).unwrap();
        assert!(n > 0, "x11vnc closed the TLS stream");
        buf.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(buf[0], 0, "expected a FramebufferUpdate, got message type {}", buf[0]);
}

/// How a wrong-password attempt ended.
#[derive(Debug)]
enum Refusal {
    /// SecurityResult "failed", with the server's reason if it sent one.
    AuthFailed(Option<String>),
    /// The server dropped the connection (EOF, unexpected EOF without TLS
    /// close_notify, or a reset) after our VNC auth response went out.
    ClosedAfterAuthResponse(String),
}

/// Runs the client handshake like [`connect`], but tracks whether the VNC
/// authentication response was sent inside TLS, so an early close can be
/// told apart from a failure at an earlier step. Panics if the server
/// accepts the password or the handshake fails anywhere else.
fn wrong_password_attempt(fx: &Fixture, password: &[u8]) -> Refusal {
    use rfb_tls::{Stream, TlsError};
    let cfg = ClientConfig { password: Some(password.to_vec()), ..ClientConfig::encrypted() };
    let mut hs = ClientHandshake::new(cfg);
    let tls = tls(fx);
    let mut stream = Stream::Plain(sock(fx.port));
    let mut buf = Vec::new();
    let mut in_tls = false;
    let mut auth_response_sent = false;
    let early_close = |e: &TlsError| match e {
        TlsError::Closed => true,
        TlsError::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        ),
        _ => false,
    };
    loop {
        let out = hs.take_output();
        if !out.is_empty() {
            stream.write_all(&out).and_then(|()| stream.flush()).expect("send handshake bytes");
            // With X509Vnc the client's first output inside TLS is the 16-byte
            // DES response to the server's challenge. Anything after it is
            // ClientInit, which only follows a successful SecurityResult.
            if in_tls {
                assert!(!auth_response_sent, "x11vnc accepted a wrong password (client sent ClientInit)");
                assert_eq!(out.len(), 16, "expected the 16-byte VNC auth response inside TLS");
                auth_response_sent = true;
            }
        }
        if hs.needs_tls().is_some() {
            stream = stream.upgrade_client(tls.config.clone(), &tls.server_name, &buf).expect("TLS 1.3 handshake");
            buf.clear();
            hs.tls_established().expect("VeNCrypt continues after TLS");
            in_tls = true;
            continue;
        }
        assert!(!hs.is_done(), "x11vnc accepted a wrong password");
        let mut chunk = [0u8; 4096];
        let read = match stream.read(&mut chunk) {
            Ok(0) => Err(TlsError::Closed),
            Ok(n) => Ok(n),
            Err(e) => Err(TlsError::Io(e)),
        };
        match read {
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if early_close(&e) && auth_response_sent => return Refusal::ClosedAfterAuthResponse(e.to_string()),
            Err(e) => panic!("connection failed before the auth response was sent (in TLS: {in_tls}): {e}"),
        }
        match hs.receive(&buf) {
            Ok(used) => drop(buf.drain(..used)),
            Err(HandshakeError::AuthFailed(reason)) => return Refusal::AuthFailed(reason),
            Err(e) => panic!("unexpected handshake error: {e}"),
        }
    }
}

#[test]
fn x509vnc_wrong_password_is_refused_by_x11vnc() {
    let Some(fx) = x11vnc_ssl(Some("s3cret")) else { return };
    // x11vnc -ssl relays through its OpenSSL helper, which sometimes drops
    // the TCP connection (no close_notify) before the SecurityResult failure
    // is flushed. Either way the session never comes up; spec/PROFILE.md
    // 10.2 says clients treat that close as an authentication failure.
    match wrong_password_attempt(&fx, b"wrong") {
        Refusal::AuthFailed(reason) => eprintln!("refused with SecurityResult: {reason:?}"),
        Refusal::ClosedAfterAuthResponse(e) => eprintln!("refused by closing after the auth response: {e}"),
    }
}

#[test]
fn x509none_with_x11vnc_and_pin_mismatch_is_rejected() {
    let Some(fx) = x11vnc_ssl(None) else { return };
    let c = connect(sock(fx.port), ClientHandshake::new(ClientConfig::encrypted()), Some(&tls(&fx))).unwrap();
    assert_eq!(c.handshake.vencrypt_subtype(), Some(VeNCryptSubtype::X509None));
    assert_eq!(c.stream.tls_info().unwrap().protocol, ProtocolVersion::TLSv1_3);

    let mut wrong = fx.cert_sha256;
    wrong[31] ^= 0xff;
    let bad = ClientTls {
        config: client_config(ServerVerification::PinnedSha256(wrong)).unwrap(),
        server_name: "localhost".into(),
    };
    let err = connect(sock(fx.port), ClientHandshake::new(ClientConfig::encrypted()), Some(&bad)).err().unwrap();
    assert!(matches!(err, rfb_tls::TlsError::Tls(rfb_tls::rustls::Error::InvalidCertificate(_))), "{err}");
}

/// A missing reference tool (or one that fails to start) skips the test,
/// unless `AURORA_REQUIRE_INTEROP=1` (set in GitHub CI), where a skip would
/// hide a broken environment and the test fails instead.
fn skip(reason: std::fmt::Arguments) {
    if std::env::var_os("AURORA_REQUIRE_INTEROP").is_some_and(|v| v == "1") {
        panic!("AURORA_REQUIRE_INTEROP=1 but {reason}");
    }
    eprintln!("SKIP: {reason}");
}
