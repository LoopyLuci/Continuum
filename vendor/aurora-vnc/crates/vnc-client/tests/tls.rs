//! The client `Session` doing VeNCrypt + TLS 1.3 against rfb-proto's server
//! handshake stacked on a real rustls server, all in memory, at several
//! network chunk sizes. Then framebuffer traffic flows through TLS.

use std::sync::{Arc, OnceLock};

use rfb_proto::*;
use rfb_tls::rustls::{self, ProtocolVersion};
use rfb_tls::{client_config, server_config, SelfSigned, ServerVerification, TlsSession};
use vnc_client::{ClientError, Event, Session, SessionOptions, TlsOptions};

const VNC_PASSWORD: &[u8] = b"vnc-pass-123";

fn cert() -> &'static SelfSigned {
    static C: OnceLock<SelfSigned> = OnceLock::new();
    C.get_or_init(|| SelfSigned::generate(&["localhost"]).unwrap())
}

fn tls_server() -> Arc<rustls::ServerConfig> {
    server_config(cert().cert_pem.as_bytes(), cert().key_pem.as_bytes()).unwrap()
}

fn client_opts(pin: [u8; 32]) -> SessionOptions {
    let tls = TlsOptions {
        config: client_config(ServerVerification::PinnedSha256(pin)).unwrap(),
        server_name: "localhost".into(),
    };
    let mut o = SessionOptions::encrypted(tls);
    o.handshake.password = Some(VNC_PASSWORD.to_vec());
    o.continuous_updates = false;
    o
}

/// Server side: ServerHandshake whose bytes go through TLS once it asks.
struct Server {
    hs: ServerHandshake,
    tls: Option<TlsSession>,
    inbuf: Vec<u8>,
}

impl Server {
    fn new() -> Self {
        let mut cfg = ServerConfig::new(ServerInit {
            width: 4,
            height: 2,
            pixel_format: PixelFormat::RGBX8888_LE,
            name: b"tls-desk".to_vec(),
        });
        cfg.vnc_auth = Some(VncAuthConfig { password: VNC_PASSWORD.to_vec(), challenge: *b"0123456789abcdef" });
        cfg.vencrypt.as_mut().unwrap().subtypes = vec![VeNCryptSubtype::X509Vnc];
        Server { hs: ServerHandshake::new(cfg), tls: None, inbuf: Vec::new() }
    }

    /// Wire bytes out.
    fn output(&mut self) -> Vec<u8> {
        let plain = self.hs.take_output();
        match &mut self.tls {
            Some(t) => {
                t.send(&plain).unwrap();
                t.take_output()
            }
            None => plain,
        }
    }

    /// Plain application bytes out (after the handshake), through TLS.
    fn send_app(&mut self, b: &[u8]) -> Vec<u8> {
        let t = self.tls.as_mut().unwrap();
        t.send(b).unwrap();
        t.take_output()
    }

    /// Wire bytes in. Returns wire bytes to send back.
    fn input(&mut self, wire: &[u8]) -> Vec<u8> {
        let mut back = Vec::new();
        let mut wire = wire.to_vec();
        loop {
            match &mut self.tls {
                Some(t) => {
                    let p = t.receive(&wire).unwrap();
                    self.inbuf.extend(p);
                    back.extend(t.take_output());
                }
                None => self.inbuf.extend_from_slice(&wire),
            }
            wire.clear();
            if !self.hs.is_done() {
                let used = self.hs.receive(&self.inbuf).unwrap();
                self.inbuf.drain(..used);
            }
            if self.hs.needs_tls().is_some() && self.tls.is_none() {
                back.extend(self.hs.take_output()); // in the clear
                self.tls = Some(TlsSession::server(tls_server()).unwrap());
                self.hs.tls_established().unwrap();
                wire = std::mem::take(&mut self.inbuf); // leftover is ciphertext
                continue;
            }
            back.extend(self.output());
            return back;
        }
    }
}

/// Runs client and server against each other, delivering `chunk` bytes at a
/// time. Returns the connected session, the server, and everything on the wire.
fn run(chunk: usize, pin: [u8; 32]) -> (Result<Session, ClientError>, Server, Vec<u8>) {
    let mut c = Session::new(client_opts(pin));
    let mut s = Server::new();
    let mut wire = Vec::new();
    let mut to_client = s.output();
    for _ in 0..10_000 {
        let mut to_server = Vec::new();
        for piece in to_client.chunks(chunk.max(1)) {
            wire.extend_from_slice(piece);
            if let Err(e) = c.receive(piece) {
                return (Err(e), s, wire);
            }
            to_server.extend(c.take_output());
        }
        to_server.extend(c.take_output());
        if c.is_connected() && s.hs.is_done() && to_server.is_empty() {
            return (Ok(c), s, wire);
        }
        wire.extend_from_slice(&to_server);
        to_client = Vec::new();
        for piece in to_server.chunks(chunk.max(1)) {
            to_client.extend(s.input(piece));
        }
        if to_client.is_empty() && to_server.is_empty() {
            break;
        }
    }
    panic!("handshake stalled at chunk size {chunk}");
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn x509vnc_over_tls13_at_every_chunk_size_then_frames_flow() {
    for chunk in [1, 2, 5, 13, 64, 1500, 1 << 20] {
        let (c, mut s, wire) = run(chunk, cert().sha256);
        let mut c = c.unwrap_or_else(|e| panic!("chunk {chunk}: {e}"));
        assert!(c.is_encrypted());
        let info = c.tls_info().expect("TLS done");
        assert_eq!(info.protocol, ProtocolVersion::TLSv1_3);
        assert_eq!(info.peer_certificate_sha256, Some(cert().sha256));
        assert!(!contains(&wire, b"tls-desk"), "desktop name leaked in the clear");

        // A Raw frame, sent through TLS.
        let mut msg = Vec::new();
        ServerMessage::FramebufferUpdate { num_rects: 1 }.encode(&mut msg);
        RectHeader { x: 0, y: 0, width: 4, height: 2, encoding: Encoding::Raw }.encode(&mut msg);
        for i in 0..8u32 {
            msg.extend_from_slice(&(0x101010 * i).to_le_bytes());
        }
        let ct = s.send_app(&msg);
        assert!(!contains(&ct, &msg[..8]), "frame not encrypted");
        let mut events = Vec::new();
        for piece in ct.chunks(chunk) {
            events.extend(c.receive(piece).unwrap());
        }
        assert!(events.iter().any(|e| matches!(e, Event::Updated { .. })), "chunk {chunk}: no update");
        assert_eq!(c.framebuffer().pixel(3, 1), 0x101010 * 7);
    }
}

#[test]
fn wrong_pin_fails_before_any_rfb_auth() {
    let (c, s, _) = run(4096, [0xab; 32]);
    assert!(matches!(c, Err(ClientError::Tls(_))), "expected TLS failure");
    assert!(!s.hs.is_done());
}

#[test]
fn vencrypt_without_tls_settings_refuses_instead_of_downgrading() {
    let mut o = client_opts(cert().sha256);
    o.tls = None;
    let mut c = Session::new(o);
    let mut s = Server::new();
    let mut to_client = s.output();
    for _ in 0..20 {
        match c.receive(&to_client) {
            Err(ClientError::Tls(m)) => {
                assert!(m.contains("no TLS settings"));
                return;
            }
            Err(e) => panic!("unexpected {e}"),
            Ok(_) => {}
        }
        to_client = s.input(&c.take_output());
    }
    panic!("client never refused");
}
