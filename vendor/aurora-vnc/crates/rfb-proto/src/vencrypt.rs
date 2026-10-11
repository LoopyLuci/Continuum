//! VeNCrypt (RFB security type 19) building blocks: subtypes, settings, and
//! Plain credentials. The negotiation itself lives in the handshake state
//! machines, so both ends share one implementation.
//!
//! Wire summary (VeNCrypt 0.2, as spoken by TigerVNC, x11vnc and libvncserver):
//!
//! ```text
//! S->C  u8 major=0, u8 minor=2           server's highest version
//! C->S  u8 major=0, u8 minor=2           version the client will speak
//! S->C  u8 0 (ok) | nonzero (refused)
//! S->C  u8 n, n x u32 subtype            subtypes on offer
//! C->S  u32 subtype                      the client's choice
//! S->C  u8 1 (ok) | 0 (refused)          then a TLS handshake starts
//! ...   inner authentication, encrypted: None, VncAuth, or Plain
//!       (C->S u32 user_len, u32 pass_len, user, pass), then SecurityResult
//! ```
//!
//! This crate never touches TLS. When negotiation reaches the TLS step the
//! handshake reports it (`needs_tls`), the caller wraps its transport, calls
//! `tls_established`, and keeps feeding the handshake decrypted bytes.

use std::fmt;

/// The only VeNCrypt version Aurora speaks. 0.1 used u8 subtypes and is obsolete.
pub const VENCRYPT_VERSION: (u8, u8) = (0, 2);

/// VeNCrypt subtypes (the registered values; unknown ones are preserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VeNCryptSubtype {
    /// 256: username and password in the clear. Never offered or accepted.
    Plain,
    /// 257-259: TLS with anonymous Diffie-Hellman (no server authentication).
    TlsNone,
    TlsVnc,
    TlsPlain,
    /// 260-262: TLS with an X.509 server certificate.
    X509None,
    X509Vnc,
    X509Plain,
    Other(u32),
}

/// Authentication that runs inside the TLS tunnel once it is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InnerAuth {
    /// No further authentication; the server answers with SecurityResult.
    None,
    /// Classic VNC challenge/response.
    VncAuth,
    /// Username and password, checked by the server application.
    Plain,
}

impl VeNCryptSubtype {
    /// The preference order Aurora uses when nothing else is configured:
    /// certificate-authenticated TLS only, strongest inner auth first.
    pub const SECURE_DEFAULT: [VeNCryptSubtype; 3] =
        [VeNCryptSubtype::X509Plain, VeNCryptSubtype::X509Vnc, VeNCryptSubtype::X509None];

    pub fn code(self) -> u32 {
        match self {
            VeNCryptSubtype::Plain => 256,
            VeNCryptSubtype::TlsNone => 257,
            VeNCryptSubtype::TlsVnc => 258,
            VeNCryptSubtype::TlsPlain => 259,
            VeNCryptSubtype::X509None => 260,
            VeNCryptSubtype::X509Vnc => 261,
            VeNCryptSubtype::X509Plain => 262,
            VeNCryptSubtype::Other(c) => c,
        }
    }

    pub fn from_code(c: u32) -> Self {
        match c {
            256 => VeNCryptSubtype::Plain,
            257 => VeNCryptSubtype::TlsNone,
            258 => VeNCryptSubtype::TlsVnc,
            259 => VeNCryptSubtype::TlsPlain,
            260 => VeNCryptSubtype::X509None,
            261 => VeNCryptSubtype::X509Vnc,
            262 => VeNCryptSubtype::X509Plain,
            c => VeNCryptSubtype::Other(c),
        }
    }

    /// Whether this subtype runs over TLS (all that Aurora will use).
    pub fn uses_tls(self) -> bool {
        self.inner().is_some()
    }

    /// TLS without server authentication (legacy anonymous Diffie-Hellman).
    /// Off by default everywhere: it is open to man-in-the-middle attacks.
    pub fn is_anonymous(self) -> bool {
        matches!(self, VeNCryptSubtype::TlsNone | VeNCryptSubtype::TlsVnc | VeNCryptSubtype::TlsPlain)
    }

    /// The authentication run inside TLS, or `None` for subtypes that do not
    /// use TLS (and so are never negotiated by Aurora).
    pub fn inner(self) -> Option<InnerAuth> {
        match self {
            VeNCryptSubtype::TlsNone | VeNCryptSubtype::X509None => Some(InnerAuth::None),
            VeNCryptSubtype::TlsVnc | VeNCryptSubtype::X509Vnc => Some(InnerAuth::VncAuth),
            VeNCryptSubtype::TlsPlain | VeNCryptSubtype::X509Plain => Some(InnerAuth::Plain),
            VeNCryptSubtype::Plain | VeNCryptSubtype::Other(_) => None,
        }
    }
}

/// Server-side VeNCrypt settings. Only present when the embedder actually has
/// a TLS configuration to wrap the connection with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VeNCryptServerConfig {
    /// Subtypes to offer, in order. Each is offered only if it can be
    /// completed: `*Vnc` needs `ServerConfig::vnc_auth`, `*Plain` needs
    /// `plain_auth`, and `Tls*` (anonymous) needs `allow_anonymous_tls`.
    pub subtypes: Vec<VeNCryptSubtype>,
    /// The embedder will check Plain credentials via
    /// `ServerHandshake::plain_credentials` and `resolve_plain`.
    pub plain_auth: bool,
    /// Offer the anonymous `Tls*` subtypes. Off by default.
    pub allow_anonymous_tls: bool,
}

impl Default for VeNCryptServerConfig {
    fn default() -> Self {
        VeNCryptServerConfig {
            subtypes: vec![
                VeNCryptSubtype::X509Plain,
                VeNCryptSubtype::X509Vnc,
                VeNCryptSubtype::X509None,
                VeNCryptSubtype::TlsPlain,
                VeNCryptSubtype::TlsVnc,
                VeNCryptSubtype::TlsNone,
            ],
            plain_auth: false,
            allow_anonymous_tls: false,
        }
    }
}

impl VeNCryptServerConfig {
    pub(crate) fn permits(&self, t: VeNCryptSubtype, vnc_auth: bool) -> bool {
        match t.inner() {
            None => false,
            Some(_) if t.is_anonymous() && !self.allow_anonymous_tls => false,
            Some(InnerAuth::None) => true,
            Some(InnerAuth::VncAuth) => vnc_auth,
            Some(InnerAuth::Plain) => self.plain_auth,
        }
    }
}

/// Credentials received through a `*Plain` subtype. The password is redacted
/// from `Debug` output and wiped when the value is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct PlainCredentials {
    pub username: Vec<u8>,
    pub password: Vec<u8>,
}

impl PlainCredentials {
    /// The VeNCrypt Plain wire form: u32 username length, u32 password
    /// length, username, password.
    pub fn encode(username: &[u8], password: &[u8], out: &mut Vec<u8>) {
        use crate::codec::{len_u32, PutBe};
        out.put_u32(len_u32(username.len(), "username"));
        out.put_u32(len_u32(password.len(), "password"));
        out.extend_from_slice(username);
        out.extend_from_slice(password);
    }

    /// Parses the Plain wire form, refusing either length above `max`
    /// before reading (or allocating) anything.
    pub fn decode(buf: &[u8], max: u32) -> Result<(Self, usize), crate::DecodeError> {
        let mut r = crate::codec::Reader::new(buf);
        let (ulen, plen) = (r.u32()?, r.u32()?);
        if ulen > max || plen > max {
            return Err(crate::DecodeError::Invalid(format!("Plain credential length exceeds limit {max}")));
        }
        let username = r.bytes(ulen as usize)?.to_vec();
        let password = r.bytes(plen as usize)?.to_vec();
        Ok((PlainCredentials { username, password }, r.consumed()))
    }

    /// Constant-time check against an expected username and password, for
    /// embedders with a fixed account list.
    pub fn matches(&self, username: &[u8], password: &[u8]) -> bool {
        let u = crate::auth::constant_time_eq(&self.username, username);
        let p = crate::auth::constant_time_eq(&self.password, password);
        u & p
    }
}

impl fmt::Debug for PlainCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlainCredentials")
            .field("username", &String::from_utf8_lossy(&self.username))
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for PlainCredentials {
    fn drop(&mut self) {
        self.password.fill(0);
    }
}
