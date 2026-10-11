//! A blocking transport that starts as plain TCP (or any `Read + Write`) and
//! upgrades itself to TLS 1.3 in the middle of a VeNCrypt handshake, plus
//! blocking drivers for both ends of the RFB handshake.
//!
//! Set read/write timeouts on the socket before handing it over; these
//! functions block on it.

use std::io::{self, Read, Write};
use std::sync::Arc;

use rfb_proto::{ClientHandshake, PlainCredentials, ServerHandshake};
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, ServerConnection, StreamOwned};

use crate::session::{info, require_tls13, TlsInfo};
use crate::TlsError;

pub enum Stream<S: Read + Write> {
    Plain(S),
    TlsClient(Box<StreamOwned<ClientConnection, S>>),
    TlsServer(Box<StreamOwned<ServerConnection, S>>),
}

macro_rules! feed_prefix {
    ($conn:expr, $prefix:expr) => {{
        let mut p: &[u8] = $prefix;
        while !p.is_empty() {
            $conn.read_tls(&mut p)?;
            $conn.process_new_packets()?;
        }
    }};
}

macro_rules! finish_handshake {
    ($conn:expr, $sock:expr) => {{
        while $conn.is_handshaking() {
            let (rd, wr) = $conn.complete_io(&mut $sock)?;
            if rd == 0 && wr == 0 && $conn.is_handshaking() && !$conn.wants_write() {
                return Err(TlsError::Closed);
            }
        }
        // Flush the client's Finished (and any tickets) now.
        while $conn.wants_write() {
            $conn.write_tls(&mut $sock)?;
        }
        require_tls13(&$conn)?;
    }};
}

impl<S: Read + Write> Stream<S> {
    /// Runs a TLS 1.3 client handshake over the plain stream. `prefix` holds
    /// bytes already read from the socket that belong to TLS.
    pub fn upgrade_client(
        self,
        config: Arc<rustls::ClientConfig>,
        server_name: &str,
        prefix: &[u8],
    ) -> Result<Self, TlsError> {
        let Stream::Plain(mut sock) = self else {
            return Err(TlsError::Tls(rustls::Error::General("stream is already TLS".into())));
        };
        let name = ServerName::try_from(server_name.to_owned())
            .map_err(|e| TlsError::Certificate(format!("invalid server name {server_name:?}: {e}")))?;
        let mut conn = ClientConnection::new(config, name)?;
        feed_prefix!(conn, prefix);
        finish_handshake!(conn, sock);
        Ok(Stream::TlsClient(Box::new(StreamOwned::new(conn, sock))))
    }

    /// Runs a TLS 1.3 server handshake over the plain stream.
    pub fn upgrade_server(self, config: Arc<rustls::ServerConfig>, prefix: &[u8]) -> Result<Self, TlsError> {
        let Stream::Plain(mut sock) = self else {
            return Err(TlsError::Tls(rustls::Error::General("stream is already TLS".into())));
        };
        let mut conn = ServerConnection::new(config)?;
        feed_prefix!(conn, prefix);
        finish_handshake!(conn, sock);
        Ok(Stream::TlsServer(Box::new(StreamOwned::new(conn, sock))))
    }

    pub fn is_tls(&self) -> bool {
        !matches!(self, Stream::Plain(_))
    }

    pub fn tls_info(&self) -> Option<TlsInfo> {
        match self {
            Stream::Plain(_) => None,
            Stream::TlsClient(s) => info(&s.conn),
            Stream::TlsServer(s) => info(&s.conn),
        }
    }

    /// The underlying transport, e.g. to change socket timeouts.
    pub fn get_ref(&self) -> &S {
        match self {
            Stream::Plain(s) => s,
            Stream::TlsClient(s) => &s.sock,
            Stream::TlsServer(s) => &s.sock,
        }
    }

    /// Sends close_notify (if TLS) and flushes.
    pub fn close(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::TlsClient(s) => {
                s.conn.send_close_notify();
                s.flush()
            }
            Stream::TlsServer(s) => {
                s.conn.send_close_notify();
                s.flush()
            }
        }
    }
}

impl<S: Read + Write> Read for Stream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::TlsClient(s) => s.read(buf),
            Stream::TlsServer(s) => s.read(buf),
        }
    }
}

impl<S: Read + Write> Write for Stream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::TlsClient(s) => s.write(buf),
            Stream::TlsServer(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::TlsClient(s) => s.flush(),
            Stream::TlsServer(s) => s.flush(),
        }
    }
}

fn send<S: Read + Write>(stream: &mut Stream<S>, out: Vec<u8>) -> Result<(), TlsError> {
    if !out.is_empty() {
        stream.write_all(&out)?;
        stream.flush()?;
    }
    Ok(())
}

fn read_more<S: Read + Write>(stream: &mut Stream<S>, buf: &mut Vec<u8>) -> Result<(), TlsError> {
    let mut chunk = [0u8; 16 * 1024];
    let n = stream.read(&mut chunk)?;
    if n == 0 {
        return Err(TlsError::Closed);
    }
    buf.extend_from_slice(&chunk[..n]);
    Ok(())
}

/// TLS settings for [`connect`].
#[derive(Clone)]
pub struct ClientTls {
    pub config: Arc<rustls::ClientConfig>,
    /// DNS name or IP address the certificate must match (unless pinned).
    pub server_name: String,
}

/// A finished client handshake.
pub struct Connected<S: Read + Write> {
    /// Plain or TLS, depending on what was negotiated.
    pub stream: Stream<S>,
    pub handshake: ClientHandshake,
    /// Bytes already received after ServerInit: the start of the message phase.
    pub leftover: Vec<u8>,
}

/// Runs the client side of the RFB handshake, upgrading to TLS if VeNCrypt
/// is negotiated. `tls` is required for VeNCrypt (`TlsError::Closed` style
/// failures are avoided by refusing early with a clear error).
pub fn connect<S: Read + Write>(
    sock: S,
    mut hs: ClientHandshake,
    tls: Option<&ClientTls>,
) -> Result<Connected<S>, TlsError> {
    let mut stream = Stream::Plain(sock);
    let mut buf = Vec::new();
    loop {
        send(&mut stream, hs.take_output())?;
        if hs.needs_tls().is_some() {
            let tls = tls.ok_or_else(|| TlsError::Certificate("VeNCrypt negotiated but no TLS config given".into()))?;
            stream = stream.upgrade_client(tls.config.clone(), &tls.server_name, &buf)?;
            buf.clear();
            hs.tls_established()?;
            continue;
        }
        if hs.is_done() {
            return Ok(Connected { stream, handshake: hs, leftover: buf });
        }
        read_more(&mut stream, &mut buf)?;
        let used = hs.receive(&buf)?;
        buf.drain(..used);
    }
}

/// A finished server handshake.
pub struct Accepted<S: Read + Write> {
    pub stream: Stream<S>,
    pub handshake: ServerHandshake,
    /// Bytes already received after ClientInit.
    pub leftover: Vec<u8>,
}

/// Runs the server side of the RFB handshake, upgrading to TLS when the
/// client picks a VeNCrypt subtype. `tls` must be `Some` whenever the
/// handshake's config offers VeNCrypt. `verify_plain` decides VeNCrypt Plain
/// logins.
pub fn accept<S: Read + Write>(
    sock: S,
    mut hs: ServerHandshake,
    tls: Option<Arc<rustls::ServerConfig>>,
    mut verify_plain: impl FnMut(&PlainCredentials) -> bool,
) -> Result<Accepted<S>, TlsError> {
    let mut stream = Stream::Plain(sock);
    let mut buf = Vec::new();
    loop {
        send(&mut stream, hs.take_output())?;
        if hs.needs_tls().is_some() {
            let tls = tls.clone().ok_or_else(|| TlsError::Certificate("VeNCrypt offered but no TLS config".into()))?;
            stream = stream.upgrade_server(tls, &buf)?;
            buf.clear();
            hs.tls_established()?;
            continue;
        }
        if let Some(creds) = hs.plain_credentials() {
            let ok = verify_plain(creds);
            let r = hs.resolve_plain(ok);
            send(&mut stream, hs.take_output())?;
            r?;
            continue;
        }
        if hs.is_done() {
            return Ok(Accepted { stream, handshake: hs, leftover: buf });
        }
        read_more(&mut stream, &mut buf)?;
        match hs.receive(&buf) {
            Ok(used) => drop(buf.drain(..used)),
            Err(e) => {
                // Best effort: let the client see the failure reason.
                let _ = send(&mut stream, hs.take_output());
                return Err(e.into());
            }
        }
    }
}
