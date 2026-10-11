use std::fmt;
use std::io;

use rfb_proto::HandshakeError;

#[derive(Debug)]
pub enum TlsError {
    /// TLS failed (bad certificate, protocol error, alert from the peer, ...).
    Tls(rustls::Error),
    Io(io::Error),
    /// A certificate, key, or fingerprint could not be parsed or generated.
    Certificate(String),
    /// The RFB handshake around TLS failed.
    Handshake(HandshakeError),
    /// The peer closed the connection before the handshake finished.
    Closed,
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Tls(e) => write!(f, "TLS: {e}"),
            TlsError::Io(e) => write!(f, "I/O: {e}"),
            TlsError::Certificate(e) => write!(f, "certificate: {e}"),
            TlsError::Handshake(e) => write!(f, "RFB handshake: {e}"),
            TlsError::Closed => write!(f, "connection closed during handshake"),
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::Tls(e) => Some(e),
            TlsError::Io(e) => Some(e),
            TlsError::Handshake(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rustls::Error> for TlsError {
    fn from(e: rustls::Error) -> Self {
        TlsError::Tls(e)
    }
}

impl From<io::Error> for TlsError {
    fn from(e: io::Error) -> Self {
        // rustls reports TLS failures inside I/O errors when driven through
        // `complete_io` or `StreamOwned`; surface them as TLS errors again.
        if e.get_ref().is_some_and(|inner| inner.is::<rustls::Error>()) {
            let inner = e.into_inner().expect("checked above");
            return TlsError::Tls(*inner.downcast::<rustls::Error>().expect("checked above"));
        }
        TlsError::Io(e)
    }
}

impl From<HandshakeError> for TlsError {
    fn from(e: HandshakeError) -> Self {
        TlsError::Handshake(e)
    }
}
