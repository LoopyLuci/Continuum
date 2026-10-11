//! End-to-end VeNCrypt through real rustls, entirely in memory: the client
//! and server handshakes from rfb-proto, each stacked on a sans-I/O
//! `TlsSession`, exchanging ciphertext through byte queues at every chunk
//! size. Proves TLS 1.3 is negotiated, the inner auth completes, and no
//! secret ever crosses the "network" in the clear.

use std::sync::{Arc, OnceLock};

use rfb_proto::*;
use rfb_tls::rustls::{self, ProtocolVersion};
use rfb_tls::{client_config, server_config, SelfSigned, ServerVerification, TlsError, TlsSession};

const PASSWORD: &[u8] = b"correct horse battery staple";
const VNC_PASSWORD: &[u8] = b"vnc-pass";
const DESKTOP: &[u8] = b"secret-desktop-name";

fn cert() -> &'static SelfSigned {
    static C: OnceLock<SelfSigned> = OnceLock::new();
    C.get_or_init(|| SelfSigned::generate(&["localhost", "127.0.0.1"]).unwrap())
}

fn tls_server() -> Arc<rustls::ServerConfig> {
    server_config(cert().cert_pem.as_bytes(), cert().key_pem.as_bytes()).unwrap()
}

fn pinned() -> Arc<rustls::ClientConfig> {
    client_config(ServerVerification::PinnedSha256(cert().sha256)).unwrap()
}

fn rfb_server() -> ServerHandshake {
    let mut cfg = ServerConfig::new(ServerInit {
        width: 800,
        height: 600,
        pixel_format: PixelFormat::RGBX8888_LE,
        name: DESKTOP.to_vec(),
    });
    cfg.vnc_auth = Some(VncAuthConfig { password: VNC_PASSWORD.to_vec(), challenge: *b"fedcba9876543210" });
    let v = cfg.vencrypt.as_mut().unwrap();
    v.plain_auth = true;
    v.subtypes = vec![VeNCryptSubtype::X509Vnc, VeNCryptSubtype::X509Plain, VeNCryptSubtype::X509None];
    ServerHandshake::new(cfg)
}

struct Outcome {
    client: ClientHandshake,
    server: ServerHandshake,
    client_tls: TlsSession,
    server_tls: TlsSession,
    /// Every byte that crossed the wire, both directions.
    wire: Vec<u8>,
}

/// One side: an RFB handshake whose bytes go through TLS once it asks for it.
struct End<H> {
    hs: H,
    tls: Option<TlsSession>,
    /// Plaintext waiting to be fed to `hs` (raw bytes before TLS starts).
    inbuf: Vec<u8>,
}

trait Hs {
    fn take_output(&mut self) -> Vec<u8>;
    fn receive(&mut self, b: &[u8]) -> Result<usize, HandshakeError>;
    fn needs_tls(&self) -> bool;
    fn tls_established(&mut self) -> Result<(), HandshakeError>;
}

macro_rules! impl_hs {
    ($t:ty) => {
        impl Hs for $t {
            fn take_output(&mut self) -> Vec<u8> {
                <$t>::take_output(self)
            }
            fn receive(&mut self, b: &[u8]) -> Result<usize, HandshakeError> {
                <$t>::receive(self, b)
            }
            fn needs_tls(&self) -> bool {
                <$t>::needs_tls(self).is_some()
            }
            fn tls_established(&mut self) -> Result<(), HandshakeError> {
                <$t>::tls_established(self)
            }
        }
    };
}
impl_hs!(ClientHandshake);
impl_hs!(ServerHandshake);

#[derive(Debug)]
enum Fail {
    Rfb(HandshakeError),
    Tls(TlsError),
}

impl<H: Hs> End<H> {
    /// Moves `hs` output onto the wire, starting TLS when asked.
    fn flush(&mut self, wire: &mut Vec<u8>, start_tls: &dyn Fn() -> TlsSession) -> Result<(), Fail> {
        let out = self.hs.take_output();
        match &mut self.tls {
            Some(t) => t.send(&out).map_err(Fail::Tls)?,
            None => wire.extend(out),
        }
        if self.tls.is_none() && self.hs.needs_tls() {
            let mut t = start_tls();
            // Raw bytes already buffered belong to TLS now.
            let early = std::mem::take(&mut self.inbuf);
            self.inbuf = t.receive(&early).map_err(Fail::Tls)?;
            self.tls = Some(t);
            self.hs.tls_established().map_err(Fail::Rfb)?;
            return self.flush(wire, &|| unreachable!());
        }
        if let Some(t) = &mut self.tls {
            wire.extend(t.take_output());
        }
        Ok(())
    }

    fn deliver(&mut self, bytes: &[u8], wire_back: &mut Vec<u8>) -> Result<(), Fail> {
        match &mut self.tls {
            Some(t) => {
                let r = t.receive(bytes);
                wire_back.extend(t.take_output());
                self.inbuf.extend(r.map_err(Fail::Tls)?);
            }
            None => self.inbuf.extend_from_slice(bytes),
        }
        let used = self.hs.receive(&self.inbuf).map_err(Fail::Rfb)?;
        self.inbuf.drain(..used);
        Ok(())
    }
}

fn run(
    client: ClientHandshake,
    client_tls: Arc<rustls::ClientConfig>,
    server_name: &str,
    chunk: usize,
) -> Result<Outcome, (Fail, Vec<u8>)> {
    let mut c = End { hs: client, tls: None, inbuf: Vec::new() };
    let mut s = End { hs: rfb_server(), tls: None, inbuf: Vec::new() };
    let (mut to_c, mut to_s, mut wire) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..100_000 {
        let step = (|| {
            c.flush(&mut to_s, &|| TlsSession::client(client_tls.clone(), server_name).unwrap())?;
            s.flush(&mut to_c, &|| TlsSession::server(tls_server()).unwrap())?;
            if let Some(creds) = s.hs.plain_credentials() {
                let ok = creds.matches(b"alice", PASSWORD);
                s.hs.resolve_plain(ok).map_err(Fail::Rfb)?;
                s.flush(&mut to_c, &|| unreachable!())?;
            }
            let n = chunk.min(to_c.len());
            let bytes: Vec<u8> = to_c.drain(..n).collect();
            wire.extend(&bytes);
            c.deliver(&bytes, &mut to_s)?;
            let n = chunk.min(to_s.len());
            let bytes: Vec<u8> = to_s.drain(..n).collect();
            wire.extend(&bytes);
            s.deliver(&bytes, &mut to_c)
        })();
        if let Err(e) = step {
            return Err((e, wire));
        }
        if c.hs.is_done() && s.hs.is_done() && to_c.is_empty() && to_s.is_empty() {
            return Ok(Outcome {
                client: c.hs,
                server: s.hs,
                client_tls: c.tls.expect("TLS was used"),
                server_tls: s.tls.expect("TLS was used"),
                wire,
            });
        }
    }
    panic!("did not converge");
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn assert_tls13(o: &Outcome) {
    let ci = o.client_tls.info().expect("client handshake complete");
    let si = o.server_tls.info().expect("server handshake complete");
    assert_eq!(ci.protocol, ProtocolVersion::TLSv1_3);
    assert_eq!(si.protocol, ProtocolVersion::TLSv1_3);
    assert_eq!(ci.cipher_suite, si.cipher_suite);
    assert_eq!(ci.peer_certificate_sha256, Some(cert().sha256));
    assert!(!contains(&o.wire, DESKTOP), "ServerInit must only travel encrypted");
}

#[test]
fn x509_plain_over_real_tls13_at_every_chunk_size() {
    for chunk in [1, 2, 3, 7, 64, 1500, 1 << 20] {
        let cfg = ClientConfig {
            username: Some(b"alice".to_vec()),
            password: Some(PASSWORD.to_vec()),
            ..ClientConfig::encrypted()
        };
        let o = run(ClientHandshake::new(cfg), pinned(), "localhost", chunk).unwrap_or_else(|e| panic!("{e:?}"));
        assert_tls13(&o);
        assert_eq!(o.client.vencrypt_subtype(), Some(VeNCryptSubtype::X509Plain));
        assert_eq!(o.server.vencrypt_subtype(), Some(VeNCryptSubtype::X509Plain));
        assert_eq!(o.client.server_init().unwrap().name, DESKTOP);
        assert!(!contains(&o.wire, PASSWORD), "password crossed the wire in the clear");
        assert!(!contains(&o.wire, b"alice"), "username crossed the wire in the clear");
    }
}

#[test]
fn x509_vnc_over_real_tls13_at_every_chunk_size() {
    for chunk in [1, 5, 1500, 1 << 20] {
        let cfg = ClientConfig { password: Some(VNC_PASSWORD.to_vec()), ..ClientConfig::encrypted() };
        let o = run(ClientHandshake::new(cfg), pinned(), "localhost", chunk).unwrap_or_else(|e| panic!("{e:?}"));
        assert_tls13(&o);
        assert_eq!(o.server.vencrypt_subtype(), Some(VeNCryptSubtype::X509Vnc));
        assert!(!contains(&o.wire, b"fedcba9876543210"), "challenge must be encrypted");
    }
}

#[test]
fn x509_none_over_real_tls13() {
    let o = run(ClientHandshake::new(ClientConfig::encrypted()), pinned(), "localhost", 4096).unwrap();
    assert_tls13(&o);
    assert_eq!(o.client.vencrypt_subtype(), Some(VeNCryptSubtype::X509None));
}

#[test]
fn wrong_plain_password_is_rejected_inside_tls() {
    let cfg = ClientConfig {
        username: Some(b"alice".to_vec()),
        password: Some(b"guess".to_vec()),
        ..ClientConfig::encrypted()
    };
    match run(ClientHandshake::new(cfg), pinned(), "localhost", 4096) {
        Err((Fail::Rfb(HandshakeError::AuthFailed(None)), _)) => {}
        other => panic!("expected server-side AuthFailed, got {:?}", other.err().map(|e| e.0)),
    }
}

#[test]
fn ca_verification_accepts_matching_name() {
    let ca = client_config(ServerVerification::CaPem(cert().cert_pem.clone().into_bytes())).unwrap();
    for name in ["localhost", "127.0.0.1"] {
        let o = run(ClientHandshake::new(ClientConfig::encrypted()), ca.clone(), name, 4096).unwrap();
        assert_tls13(&o);
    }
}

#[test]
fn ca_verification_rejects_wrong_name() {
    let ca = client_config(ServerVerification::CaPem(cert().cert_pem.clone().into_bytes())).unwrap();
    match run(ClientHandshake::new(ClientConfig::encrypted()), ca, "evil.example", 4096) {
        Err((Fail::Tls(TlsError::Tls(rustls::Error::InvalidCertificate(_))), _)) => {}
        other => panic!("expected certificate rejection, got {:?}", other.err().map(|e| e.0)),
    }
}

#[test]
fn wrong_pin_is_rejected_before_any_secret_is_sent() {
    let mut fp = cert().sha256;
    fp[0] ^= 1;
    let bad = client_config(ServerVerification::PinnedSha256(fp)).unwrap();
    let cfg = ClientConfig {
        username: Some(b"alice".to_vec()),
        password: Some(PASSWORD.to_vec()),
        ..ClientConfig::encrypted()
    };
    match run(ClientHandshake::new(cfg), bad, "localhost", 4096) {
        Err((Fail::Tls(TlsError::Tls(rustls::Error::InvalidCertificate(_))), wire)) => {
            assert!(!contains(&wire, PASSWORD));
        }
        other => panic!("expected pin mismatch, got {:?}", other.err().map(|e| e.0)),
    }
}

#[test]
fn unknown_ca_is_rejected() {
    let other = SelfSigned::generate(&["localhost"]).unwrap();
    let ca = client_config(ServerVerification::CaPem(other.cert_pem.into_bytes())).unwrap();
    assert!(matches!(
        run(ClientHandshake::new(ClientConfig::encrypted()), ca, "localhost", 4096),
        Err((Fail::Tls(TlsError::Tls(rustls::Error::InvalidCertificate(_))), _))
    ));
}

#[test]
fn insecure_mode_still_encrypts_with_tls13() {
    let any = client_config(ServerVerification::InsecureAcceptAny).unwrap();
    let o = run(ClientHandshake::new(ClientConfig::encrypted()), any, "whatever.invalid", 4096).unwrap();
    assert_tls13(&o);
}

#[test]
fn plaintext_larger_than_the_rustls_send_buffer_goes_through() {
    let mut c = TlsSession::client(pinned(), "localhost").unwrap();
    let mut s = TlsSession::server(tls_server()).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7) as u8).collect();
    s.send(&big).unwrap(); // queued before the handshake finishes
    let mut got = Vec::new();
    for _ in 0..20 {
        let to_s = c.take_output();
        got.extend(s.receive(&to_s).unwrap());
        let to_c = s.take_output();
        got.extend(c.receive(&to_c).unwrap());
        if got.len() >= big.len() {
            break;
        }
    }
    assert!(got == big, "got {} of {} bytes", got.len(), big.len());
    s.send(&big).unwrap(); // and after it
    let got = c.receive(&s.take_output()).unwrap();
    assert!(got == big);
}
