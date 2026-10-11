//! `rfb-tls`: TLS 1.3 for Aurora VNC, built on rustls with the `ring`
//! provider. It is the only crate in the workspace that knows about TLS;
//! `rfb-proto` stays pure and only tells the caller *when* to start TLS
//! (`needs_tls`) for VeNCrypt.
//!
//! - [`config`]: client and server `rustls` configurations, TLS 1.3 only,
//!   with CA, system-root, pinned-certificate, or explicit insecure verification.
//! - [`cert`]: self-signed certificates for first-run hosts, and SHA-256
//!   fingerprints for pinning.
//! - [`TlsSession`]: a sans-I/O TLS endpoint (ciphertext in, plaintext out)
//!   for event loops and in-memory tests.
//! - [`Stream`] plus [`connect`] / [`accept`]: a blocking transport that
//!   upgrades itself in the middle of a VeNCrypt handshake.

#![forbid(unsafe_code)]

pub mod cert;
pub mod config;
mod error;
mod session;
mod stream;

pub use cert::{fingerprint_sha256, format_fingerprint, parse_fingerprint, SelfSigned};
pub use config::{client_config, server_config, ServerVerification};
pub use error::TlsError;
pub use session::{TlsInfo, TlsSession};
pub use stream::{accept, connect, Accepted, ClientTls, Connected, Stream};

/// Re-exported so embedders use the exact rustls version this crate was built with.
pub use rustls;
