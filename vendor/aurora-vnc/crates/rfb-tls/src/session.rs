//! A sans-I/O TLS endpoint: the same "bytes in, bytes out" shape as the
//! `rfb-proto` state machines, so an event loop (or an in-memory test) can
//! stack the two without blocking sockets.

use std::io::{self, Read, Write};
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{CipherSuite, Connection, ProtocolVersion};

use crate::cert::fingerprint_sha256;
use crate::TlsError;

/// What was negotiated, for logs, UIs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsInfo {
    pub protocol: ProtocolVersion,
    pub cipher_suite: CipherSuite,
    /// SHA-256 of the peer's leaf certificate (servers always have one).
    pub peer_certificate_sha256: Option<[u8; 32]>,
}

pub(crate) fn info(c: &rustls::CommonState) -> Option<TlsInfo> {
    if c.is_handshaking() {
        return None;
    }
    Some(TlsInfo {
        protocol: c.protocol_version()?,
        cipher_suite: c.negotiated_cipher_suite()?.suite(),
        peer_certificate_sha256: c.peer_certificates().and_then(|certs| certs.first()).map(|c| fingerprint_sha256(c)),
    })
}

pub(crate) fn require_tls13(c: &rustls::CommonState) -> Result<(), TlsError> {
    match c.protocol_version() {
        Some(ProtocolVersion::TLSv1_3) => Ok(()),
        other => Err(TlsError::Tls(rustls::Error::General(format!("negotiated {other:?}, not TLS 1.3")))),
    }
}

/// A TLS 1.3 endpoint with no I/O of its own.
///
/// ```text
///   network bytes --> receive --> plaintext (for the RFB state machine)
///   plaintext --> send ; take_output --> network bytes
/// ```
pub struct TlsSession {
    conn: Connection,
    peer_closed: bool,
}

impl TlsSession {
    /// A client endpoint. `server_name` is a DNS name or an IP address; it is
    /// what the certificate is checked against (unless pinned).
    pub fn client(config: Arc<rustls::ClientConfig>, server_name: &str) -> Result<Self, TlsError> {
        let name = ServerName::try_from(server_name.to_owned())
            .map_err(|e| TlsError::Certificate(format!("invalid server name {server_name:?}: {e}")))?;
        let mut conn = rustls::ClientConnection::new(config, name)?;
        // No I/O of our own: the caller drains `take_output`, so plaintext
        // larger than rustls' default 64 KiB send buffer must not fail.
        conn.set_buffer_limit(None);
        Ok(TlsSession { conn: Connection::Client(conn), peer_closed: false })
    }

    pub fn server(config: Arc<rustls::ServerConfig>) -> Result<Self, TlsError> {
        let mut conn = rustls::ServerConnection::new(config)?;
        conn.set_buffer_limit(None);
        Ok(TlsSession { conn: Connection::Server(conn), peer_closed: false })
    }

    /// Feeds bytes received from the network and returns the plaintext they
    /// completed. On error, call `take_output` once more to send the alert.
    pub fn receive(&mut self, mut ciphertext: &[u8]) -> Result<Vec<u8>, TlsError> {
        let mut plain = Vec::new();
        while !ciphertext.is_empty() {
            let n = self.conn.read_tls(&mut ciphertext)?;
            let state = self.conn.process_new_packets()?;
            if state.plaintext_bytes_to_read() > 0 || state.peer_has_closed() {
                self.drain_plaintext(&mut plain)?;
            }
            if n == 0 {
                break;
            }
        }
        Ok(plain)
    }

    fn drain_plaintext(&mut self, plain: &mut Vec<u8>) -> Result<(), TlsError> {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match self.conn.reader().read(&mut buf) {
                Ok(0) => {
                    self.peer_closed = true;
                    return Ok(());
                }
                Ok(n) => plain.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Queues plaintext. Data sent before the handshake finishes is held
    /// back and encrypted once it does.
    pub fn send(&mut self, plaintext: &[u8]) -> Result<(), TlsError> {
        self.conn.writer().write_all(plaintext)?;
        Ok(())
    }

    /// Bytes to write to the network.
    pub fn take_output(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while self.conn.wants_write() {
            if self.conn.write_tls(&mut out).is_err() {
                break;
            }
        }
        out
    }

    pub fn is_handshaking(&self) -> bool {
        self.conn.is_handshaking()
    }

    /// The peer sent close_notify.
    pub fn peer_closed(&self) -> bool {
        self.peer_closed
    }

    /// Queues close_notify; flush it with `take_output`.
    pub fn close(&mut self) {
        self.conn.send_close_notify();
    }

    /// Negotiated parameters, once the handshake is complete.
    pub fn info(&self) -> Option<TlsInfo> {
        info(&self.conn)
    }
}
