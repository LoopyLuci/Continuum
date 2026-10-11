//! Sans-I/O handshake state machines for both ends of an RFB connection,
//! covering ProtocolVersion, security negotiation, and initialisation
//! (RFC 6143 sections 7.1 to 7.3).
//!
//! Usage on either side: append received bytes to a buffer, call `receive`,
//! drop the consumed prefix, write out whatever `take_output` returns, and
//! repeat until `is_done`. Any bytes left over after completion belong to
//! the normal message phase.
//!
//! Security types implemented here: None, VncAuth, and VeNCrypt (type 19)
//! with the TLS subtypes. Others are refused cleanly rather than half-handled.
//! Rate limiting of failed logins is the host's job, not this crate's.
//!
//! VeNCrypt needs one extra step from the caller, because this crate never
//! does TLS itself. When `needs_tls` returns a subtype:
//!
//! 1. write out everything `take_output` returns, unencrypted;
//! 2. start TLS on the transport (client or server side). Any bytes already
//!    read but not consumed by `receive` belong to the TLS stream;
//! 3. call `tls_established`, then carry on as before, except that all bytes
//!    now go through TLS: decrypted input in, `take_output` encrypted out.

use std::fmt;

use crate::auth::{constant_time_eq, vnc_auth_response, CHALLENGE_LEN};
use crate::codec::{DecodeError, Limits, PutBe, Reader};
use crate::types::{ClientInit, SecurityType, ServerInit, Version};
use crate::vencrypt::{InnerAuth, PlainCredentials, VeNCryptServerConfig, VeNCryptSubtype, VENCRYPT_VERSION};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    Decode(DecodeError),
    /// The server refused the connection and gave this reason.
    Refused(String),
    /// No security type acceptable to both sides.
    NoCommonSecurity {
        offered: Vec<SecurityType>,
    },
    /// The peer chose a security type we did not offer.
    UnexpectedSecurity(SecurityType),
    /// Authentication failed (3.8 servers include a reason).
    AuthFailed(Option<String>),
    /// `receive` was called after the handshake finished or failed.
    Finished,
    /// The peer speaks a VeNCrypt version we do not (only 0.2 is supported).
    VeNCryptVersion {
        major: u8,
        minor: u8,
    },
    /// No VeNCrypt subtype acceptable to both sides.
    NoCommonSubtype {
        offered: Vec<VeNCryptSubtype>,
    },
    /// The client chose a VeNCrypt subtype the server did not offer.
    UnexpectedSubtype(VeNCryptSubtype),
    /// The server declined to start TLS after the subtype was chosen.
    TlsRefused,
    /// `tls_established` or `resolve_plain` was called at the wrong time.
    InvalidState(&'static str),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandshakeError::Decode(e) => write!(f, "{e}"),
            HandshakeError::Refused(r) => write!(f, "server refused connection: {r}"),
            HandshakeError::NoCommonSecurity { offered } => {
                write!(f, "no common security type (server offered {offered:?})")
            }
            HandshakeError::UnexpectedSecurity(t) => write!(f, "peer chose security type {t:?}, which was not offered"),
            HandshakeError::AuthFailed(Some(r)) => write!(f, "authentication failed: {r}"),
            HandshakeError::AuthFailed(None) => write!(f, "authentication failed"),
            HandshakeError::Finished => write!(f, "handshake already finished"),
            HandshakeError::VeNCryptVersion { major, minor } => {
                write!(f, "unsupported VeNCrypt version {major}.{minor} (need 0.2)")
            }
            HandshakeError::NoCommonSubtype { offered } => {
                write!(f, "no common VeNCrypt subtype (server offered {offered:?})")
            }
            HandshakeError::UnexpectedSubtype(t) => {
                write!(f, "peer chose VeNCrypt subtype {t:?}, which was not offered")
            }
            HandshakeError::TlsRefused => write!(f, "server declined to start TLS"),
            HandshakeError::InvalidState(what) => write!(f, "handshake misuse: {what}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<DecodeError> for HandshakeError {
    fn from(e: DecodeError) -> Self {
        HandshakeError::Decode(e)
    }
}

/// Run `step` repeatedly over `input`, stopping cleanly on Incomplete.
fn drive<S>(
    this: &mut S,
    input: &[u8],
    mut step: impl FnMut(&mut S, &[u8]) -> Result<Option<usize>, HandshakeError>,
) -> Result<usize, HandshakeError> {
    let mut used = 0;
    loop {
        match step(this, &input[used..]) {
            Ok(Some(n)) => used += n,
            Ok(None) => return Ok(used),
            Err(HandshakeError::Decode(DecodeError::Incomplete { .. })) => return Ok(used),
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------- client

#[derive(Clone)]
pub struct ClientConfig {
    /// Highest version to speak. We answer with min(this, server's version).
    pub max_version: Version,
    /// Acceptable security types, most preferred first.
    pub security: Vec<SecurityType>,
    pub shared: bool,
    /// Password for VncAuth and VeNCrypt Plain. VncAuth is never chosen when
    /// this is `None`.
    pub password: Option<Vec<u8>>,
    /// Username for VeNCrypt `*Plain` subtypes (which also need `password`).
    pub username: Option<Vec<u8>>,
    /// VeNCrypt subtypes to accept, most preferred first. Used only when
    /// `security` contains `SecurityType::VeNCrypt`.
    pub vencrypt_subtypes: Vec<VeNCryptSubtype>,
    /// Accept the anonymous `Tls*` subtypes (no server authentication,
    /// open to man-in-the-middle attacks). Off by default.
    pub allow_anonymous_tls: bool,
    pub limits: Limits,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            max_version: Version::V3_8,
            security: vec![SecurityType::None],
            shared: true,
            password: None,
            username: None,
            vencrypt_subtypes: vec![
                VeNCryptSubtype::X509Plain,
                VeNCryptSubtype::X509Vnc,
                VeNCryptSubtype::X509None,
                VeNCryptSubtype::TlsPlain,
                VeNCryptSubtype::TlsVnc,
                VeNCryptSubtype::TlsNone,
            ],
            allow_anonymous_tls: false,
            limits: Limits::default(),
        }
    }
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("max_version", &self.max_version)
            .field("security", &self.security)
            .field("shared", &self.shared)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("username", &self.username.as_ref().map(|u| String::from_utf8_lossy(u)))
            .field("vencrypt_subtypes", &self.vencrypt_subtypes)
            .field("allow_anonymous_tls", &self.allow_anonymous_tls)
            .field("limits", &self.limits)
            .finish()
    }
}

impl ClientConfig {
    /// A client that only accepts VeNCrypt with certificate-authenticated
    /// TLS (the subtypes in `VeNCryptSubtype::SECURE_DEFAULT`). Set
    /// `password` (and `username` for Plain) as needed.
    pub fn encrypted() -> Self {
        ClientConfig {
            security: crate::compat::SECURITY_LADDER.to_vec(),
            allow_anonymous_tls: true,
            ..Default::default()
        }
    }

    fn can_do(&self, t: SecurityType) -> bool {
        self.security.contains(&t)
            && match t {
                SecurityType::None => true,
                SecurityType::VncAuth => self.password.is_some(),
                SecurityType::VeNCrypt => self.vencrypt_subtypes.iter().any(|&s| self.can_do_subtype(s)),
                _ => false,
            }
    }

    fn can_do_subtype(&self, t: VeNCryptSubtype) -> bool {
        match t.inner() {
            None => false,
            Some(_) if t.is_anonymous() && !self.allow_anonymous_tls => false,
            Some(InnerAuth::None) => true,
            Some(InnerAuth::VncAuth) => self.password.is_some(),
            Some(InnerAuth::Plain) => self.password.is_some() && self.username.is_some(),
        }
    }
}

#[derive(Debug)]
enum ClientState {
    AwaitVersion,
    AwaitSecurityTypes,
    AwaitSecurityType33,
    AwaitVncChallenge,
    AwaitVeNCryptVersion,
    AwaitVeNCryptAck,
    AwaitVeNCryptSubtypes,
    AwaitTlsAck(VeNCryptSubtype),
    AwaitTls(VeNCryptSubtype),
    AwaitSecurityResult,
    AwaitServerInit,
    Done(ServerInit),
    Failed,
}

pub struct ClientHandshake {
    cfg: ClientConfig,
    state: ClientState,
    version: Option<Version>,
    security: Option<SecurityType>,
    subtype: Option<VeNCryptSubtype>,
    out: Vec<u8>,
}

impl ClientHandshake {
    pub fn new(cfg: ClientConfig) -> Self {
        ClientHandshake {
            cfg,
            state: ClientState::AwaitVersion,
            version: None,
            security: None,
            subtype: None,
            out: Vec::new(),
        }
    }

    /// The negotiated VeNCrypt subtype, once chosen.
    pub fn vencrypt_subtype(&self) -> Option<VeNCryptSubtype> {
        self.subtype
    }

    /// `Some(subtype)` when the handshake is paused waiting for the caller to
    /// start TLS (client side). Flush `take_output` unencrypted first. For
    /// X.509 subtypes the server certificate must be verified; for the
    /// anonymous `Tls*` subtypes (only possible when `allow_anonymous_tls`
    /// is set) it cannot be.
    pub fn needs_tls(&self) -> Option<VeNCryptSubtype> {
        match self.state {
            ClientState::AwaitTls(t) => Some(t),
            _ => None,
        }
    }

    /// Tell the handshake that TLS is up. From now on every byte in and out
    /// must go through TLS. May queue output (Plain credentials).
    pub fn tls_established(&mut self) -> Result<(), HandshakeError> {
        let ClientState::AwaitTls(t) = self.state else {
            return Err(HandshakeError::InvalidState("tls_established called when TLS was not requested"));
        };
        match t.inner().expect("only TLS subtypes are chosen") {
            InnerAuth::None => self.state = ClientState::AwaitSecurityResult,
            InnerAuth::VncAuth => self.state = ClientState::AwaitVncChallenge,
            InnerAuth::Plain => {
                let user = self.cfg.username.as_deref().unwrap_or_default();
                let pass = self.cfg.password.as_deref().unwrap_or_default();
                PlainCredentials::encode(user, pass, &mut self.out);
                self.state = ClientState::AwaitSecurityResult;
            }
        }
        Ok(())
    }

    pub fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn security_type(&self) -> Option<SecurityType> {
        self.security
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, ClientState::Done(_))
    }

    pub fn server_init(&self) -> Option<&ServerInit> {
        match &self.state {
            ClientState::Done(si) => Some(si),
            _ => None,
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Feed bytes from the server. Returns how many were consumed.
    pub fn receive(&mut self, input: &[u8]) -> Result<usize, HandshakeError> {
        let r = drive(self, input, Self::step);
        if r.is_err() {
            self.state = ClientState::Failed;
        }
        r
    }

    fn after_security(&mut self) {
        self.state = ClientState::AwaitServerInit;
        ClientInit { shared: self.cfg.shared }.encode(&mut self.out);
    }

    fn step(&mut self, input: &[u8]) -> Result<Option<usize>, HandshakeError> {
        let limits = self.cfg.limits;
        match self.state {
            ClientState::AwaitVersion => {
                let (server_v, n) = Version::decode(input)?;
                let v = server_v.min(self.cfg.max_version);
                self.version = Some(v);
                v.encode(&mut self.out);
                self.state =
                    if v == Version::V3_3 { ClientState::AwaitSecurityType33 } else { ClientState::AwaitSecurityTypes };
                Ok(Some(n))
            }
            ClientState::AwaitSecurityTypes => {
                let mut r = Reader::new(input);
                let count = r.u8()?;
                if count == 0 {
                    let reason = r.len_prefixed(limits.max_reason_len, "failure reason")?;
                    return Err(HandshakeError::Refused(String::from_utf8_lossy(reason).into_owned()));
                }
                let offered: Vec<SecurityType> =
                    r.bytes(count as usize)?.iter().map(|&c| SecurityType::from_code(c)).collect();
                let chosen = self.cfg.security.iter().copied().find(|t| offered.contains(t) && self.cfg.can_do(*t));
                let Some(chosen) = chosen else {
                    return Err(HandshakeError::NoCommonSecurity { offered });
                };
                self.out.put_u8(chosen.code());
                self.security = Some(chosen);
                match chosen {
                    SecurityType::None if self.version.is_some_and(Version::uses_38_security) => {
                        self.state = ClientState::AwaitSecurityResult
                    }
                    SecurityType::None => self.after_security(),
                    SecurityType::VncAuth => self.state = ClientState::AwaitVncChallenge,
                    SecurityType::VeNCrypt => self.state = ClientState::AwaitVeNCryptVersion,
                    _ => unreachable!("can_do admits only implemented types"),
                }
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitSecurityType33 => {
                let mut r = Reader::new(input);
                let t = SecurityType::from_code(u8::try_from(r.u32()?).unwrap_or(255));
                match t {
                    SecurityType::Invalid => {
                        let reason = r.len_prefixed(limits.max_reason_len, "failure reason")?;
                        return Err(HandshakeError::Refused(String::from_utf8_lossy(reason).into_owned()));
                    }
                    t if !self.cfg.can_do(t) => return Err(HandshakeError::NoCommonSecurity { offered: vec![t] }),
                    SecurityType::None => {
                        self.security = Some(t);
                        self.after_security();
                    }
                    SecurityType::VncAuth => {
                        self.security = Some(t);
                        self.state = ClientState::AwaitVncChallenge;
                    }
                    SecurityType::VeNCrypt => {
                        self.security = Some(t);
                        self.state = ClientState::AwaitVeNCryptVersion;
                    }
                    _ => unreachable!("can_do admits only implemented types"),
                }
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitVncChallenge => {
                let mut r = Reader::new(input);
                let challenge: [u8; CHALLENGE_LEN] = r.bytes(CHALLENGE_LEN)?.try_into().expect("16 bytes");
                let password = self.cfg.password.as_deref().unwrap_or_default();
                self.out.extend_from_slice(&vnc_auth_response(password, &challenge));
                self.state = ClientState::AwaitSecurityResult;
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitVeNCryptVersion => {
                let mut r = Reader::new(input);
                let (major, minor) = (r.u8()?, r.u8()?);
                if (major, minor) < VENCRYPT_VERSION {
                    return Err(HandshakeError::VeNCryptVersion { major, minor });
                }
                self.out.put_u8(VENCRYPT_VERSION.0);
                self.out.put_u8(VENCRYPT_VERSION.1);
                self.state = ClientState::AwaitVeNCryptAck;
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitVeNCryptAck => {
                let mut r = Reader::new(input);
                if r.u8()? != 0 {
                    return Err(HandshakeError::Refused("server rejected VeNCrypt version 0.2".into()));
                }
                self.state = ClientState::AwaitVeNCryptSubtypes;
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitVeNCryptSubtypes => {
                let mut r = Reader::new(input);
                let count = r.u8()?;
                if count > limits.max_vencrypt_subtypes {
                    return Err(DecodeError::Invalid(format!(
                        "{count} VeNCrypt subtypes exceeds limit {}",
                        limits.max_vencrypt_subtypes
                    ))
                    .into());
                }
                let mut offered = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    offered.push(VeNCryptSubtype::from_code(r.u32()?));
                }
                let chosen = self
                    .cfg
                    .vencrypt_subtypes
                    .iter()
                    .copied()
                    .find(|t| offered.contains(t) && self.cfg.can_do_subtype(*t));
                let Some(chosen) = chosen else {
                    return Err(HandshakeError::NoCommonSubtype { offered });
                };
                self.out.put_u32(chosen.code());
                self.subtype = Some(chosen);
                self.state = ClientState::AwaitTlsAck(chosen);
                Ok(Some(r.consumed()))
            }
            ClientState::AwaitTlsAck(t) => {
                let mut r = Reader::new(input);
                if r.u8()? == 0 {
                    return Err(HandshakeError::TlsRefused);
                }
                self.state = ClientState::AwaitTls(t);
                Ok(Some(r.consumed()))
            }
            // Everything after the ack belongs to TLS; consume nothing.
            ClientState::AwaitTls(_) => Ok(None),
            ClientState::AwaitSecurityResult => {
                let mut r = Reader::new(input);
                match r.u32()? {
                    0 => {
                        self.after_security();
                        Ok(Some(r.consumed()))
                    }
                    _ => {
                        let reason = if self.version.is_some_and(Version::uses_38_security) {
                            Some(
                                String::from_utf8_lossy(r.len_prefixed(limits.max_reason_len, "failure reason")?)
                                    .into_owned(),
                            )
                        } else {
                            None
                        };
                        Err(HandshakeError::AuthFailed(reason))
                    }
                }
            }
            ClientState::AwaitServerInit => {
                let (si, n) = ServerInit::decode(input, &limits)?;
                self.state = ClientState::Done(si);
                Ok(Some(n))
            }
            ClientState::Done(_) => Ok(None),
            ClientState::Failed => Err(HandshakeError::Finished),
        }
    }
}

// ---------------------------------------------------------------- server

/// Server-side VncAuth settings. The challenge must come from a
/// cryptographically secure RNG and be fresh for every connection; this crate
/// takes it as input so it stays deterministic and testable.
#[derive(Clone)]
pub struct VncAuthConfig {
    pub password: Vec<u8>,
    pub challenge: [u8; CHALLENGE_LEN],
}

impl fmt::Debug for VncAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VncAuthConfig").field("password", &"<redacted>").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub version: Version,
    /// Security types to offer, most preferred first (3.3 clients get the
    /// first one the server can complete).
    pub security: Vec<SecurityType>,
    pub server_init: ServerInit,
    /// Required for VncAuth (top-level or inside VeNCrypt) to be offered.
    pub vnc_auth: Option<VncAuthConfig>,
    /// Required for VeNCrypt to be offered. Set it only when the embedder has
    /// a TLS configuration to wrap the connection with.
    pub vencrypt: Option<VeNCryptServerConfig>,
    pub limits: Limits,
}

impl ServerConfig {
    /// Secure defaults: RFB 3.8, VeNCrypt only (X509Vnc then X509Plain).
    /// Nothing is offerable until the embedder adds `vnc_auth` or sets
    /// `vencrypt.plain_auth`; unencrypted None/VncAuth must be added to
    /// `security` explicitly.
    pub fn new(server_init: ServerInit) -> Self {
        ServerConfig {
            version: Version::V3_8,
            // VeNCrypt only, not `compat::SECURITY_LADDER`.
            //
            // The ladder is `[VeNCrypt, VncAuth]`, and seeding from it made the
            // default offer VNC Auth -- an unencrypted, password-only scheme --
            // to every client, which is the opposite of what the secure default
            // documented directly above promises. `offerable()` would have
            // filtered it out on a correctly gated path, but the type list is
            // also written on paths that do not re-filter, so the byte went out
            // regardless: a 3.8 client saw two security types where there is one,
            // and the 3.7 path differed by exactly the five bytes that extra
            // entry costs.
            //
            // An embedder that genuinely wants VncAuth adds it explicitly, which
            // is what the last line of the doc comment says to do.
            security: vec![SecurityType::VeNCrypt],
            server_init,
            vnc_auth: None,
            vencrypt: Some(VeNCryptServerConfig::default()),
            limits: Limits::default(),
        }
    }

    /// Security types that are both configured and implemented, in order.
    fn offerable(&self) -> Vec<SecurityType> {
        self.security
            .iter()
            .copied()
            .filter(|t| match t {
                SecurityType::None => true,
                SecurityType::VncAuth => self.vnc_auth.is_some(),
                SecurityType::VeNCrypt => !self.offerable_subtypes().is_empty(),
                _ => false,
            })
            .collect()
    }

    /// VeNCrypt subtypes that are both configured and completable, in order.
    fn offerable_subtypes(&self) -> Vec<VeNCryptSubtype> {
        let Some(v) = &self.vencrypt else { return Vec::new() };
        let mut out: Vec<VeNCryptSubtype> = Vec::new();
        for &t in &v.subtypes {
            if v.permits(t, self.vnc_auth.is_some()) && !out.contains(&t) {
                out.push(t);
            }
        }
        out.truncate(usize::from(self.limits.max_vencrypt_subtypes));
        out
    }
}

#[derive(Debug)]
enum ServerState {
    AwaitVersion,
    AwaitSecurityChoice,
    AwaitVncResponse,
    AwaitVeNCryptVersion,
    AwaitVeNCryptSubtype,
    AwaitTls(VeNCryptSubtype),
    AwaitPlainCredentials,
    AwaitPlainVerdict(PlainCredentials),
    AwaitClientInit,
    Done(ClientInit),
    Failed,
}

pub struct ServerHandshake {
    cfg: ServerConfig,
    state: ServerState,
    version: Option<Version>,
    security: Option<SecurityType>,
    subtype: Option<VeNCryptSubtype>,
    out: Vec<u8>,
}

impl ServerHandshake {
    /// Creates the handshake and queues the server's ProtocolVersion.
    pub fn new(cfg: ServerConfig) -> Self {
        let mut out = Vec::new();
        cfg.version.encode(&mut out);
        ServerHandshake { cfg, state: ServerState::AwaitVersion, version: None, security: None, subtype: None, out }
    }

    /// The negotiated VeNCrypt subtype, once chosen.
    pub fn vencrypt_subtype(&self) -> Option<VeNCryptSubtype> {
        self.subtype
    }

    /// `Some(subtype)` when the handshake is paused waiting for the caller to
    /// start TLS (server side). Flush `take_output` unencrypted first: it
    /// ends with the ack byte the client waits for before its ClientHello.
    pub fn needs_tls(&self) -> Option<VeNCryptSubtype> {
        match self.state {
            ServerState::AwaitTls(t) => Some(t),
            _ => None,
        }
    }

    /// Tell the handshake that TLS is up. Queues the inner authentication's
    /// first message (VncAuth challenge or SecurityResult), to be sent
    /// through TLS.
    pub fn tls_established(&mut self) -> Result<(), HandshakeError> {
        let ServerState::AwaitTls(t) = self.state else {
            return Err(HandshakeError::InvalidState("tls_established called when TLS was not requested"));
        };
        match t.inner().expect("only TLS subtypes are offered") {
            InnerAuth::None => {
                // VeNCrypt always sends SecurityResult, even for 3.7.
                self.out.put_u32(0);
                self.state = ServerState::AwaitClientInit;
            }
            InnerAuth::VncAuth => {
                let auth = self.cfg.vnc_auth.as_ref().expect("*Vnc only offered when VncAuth is configured");
                self.out.extend_from_slice(&auth.challenge);
                self.state = ServerState::AwaitVncResponse;
            }
            InnerAuth::Plain => self.state = ServerState::AwaitPlainCredentials,
        }
        Ok(())
    }

    /// Credentials sent by the client through a `*Plain` subtype, waiting for
    /// the embedder's verdict via `resolve_plain`.
    pub fn plain_credentials(&self) -> Option<&PlainCredentials> {
        match &self.state {
            ServerState::AwaitPlainVerdict(c) => Some(c),
            _ => None,
        }
    }

    /// Accept or reject the pending Plain credentials. Rejection queues a
    /// failed SecurityResult and returns `AuthFailed`.
    pub fn resolve_plain(&mut self, accept: bool) -> Result<(), HandshakeError> {
        if !matches!(self.state, ServerState::AwaitPlainVerdict(_)) {
            return Err(HandshakeError::InvalidState("resolve_plain called with no credentials pending"));
        }
        if accept {
            self.out.put_u32(0);
            self.state = ServerState::AwaitClientInit;
            Ok(())
        } else {
            self.fail_security("authentication failed");
            self.state = ServerState::Failed;
            Err(HandshakeError::AuthFailed(None))
        }
    }

    pub fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn security_type(&self) -> Option<SecurityType> {
        self.security
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, ServerState::Done(_))
    }

    pub fn client_init(&self) -> Option<ClientInit> {
        match self.state {
            ServerState::Done(ci) => Some(ci),
            _ => None,
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Feed bytes from the client. Returns how many were consumed.
    pub fn receive(&mut self, input: &[u8]) -> Result<usize, HandshakeError> {
        let r = drive(self, input, Self::step);
        if r.is_err() {
            self.state = ServerState::Failed;
        }
        r
    }

    /// Start the chosen security type; only called with offerable types.
    fn begin_security(&mut self, t: SecurityType) {
        match t {
            SecurityType::None => {
                if self.version.is_some_and(Version::uses_38_security) {
                    self.out.put_u32(0);
                }
                self.state = ServerState::AwaitClientInit;
            }
            SecurityType::VncAuth => {
                let auth = self.cfg.vnc_auth.as_ref().expect("VncAuth only offered when configured");
                self.out.extend_from_slice(&auth.challenge);
                self.state = ServerState::AwaitVncResponse;
            }
            SecurityType::VeNCrypt => {
                self.out.put_u8(VENCRYPT_VERSION.0);
                self.out.put_u8(VENCRYPT_VERSION.1);
                self.state = ServerState::AwaitVeNCryptVersion;
            }
            _ => unreachable!("offerable admits only implemented types"),
        }
    }

    fn fail_security(&mut self, reason: &str) {
        self.out.put_u32(1);
        if self.version.is_some_and(Version::uses_38_security) {
            self.out.put_u32(reason.len() as u32);
            self.out.extend_from_slice(reason.as_bytes());
        }
    }

    fn step(&mut self, input: &[u8]) -> Result<Option<usize>, HandshakeError> {
        match self.state {
            ServerState::AwaitVersion => {
                let (client_v, n) = Version::decode(input)?;
                let v = client_v.min(self.cfg.version);
                self.version = Some(v);
                if v == Version::V3_3 {
                    // 3.3: the server picks. Only types we can complete are offered.
                    let t = self.cfg.offerable().first().copied();
                    let Some(t) = t else {
                        let reason = b"no security type usable with RFB 3.3";
                        self.out.put_u32(0);
                        self.out.put_u32(reason.len() as u32);
                        self.out.extend_from_slice(reason);
                        return Err(HandshakeError::NoCommonSecurity { offered: self.cfg.security.clone() });
                    };
                    self.out.put_u32(t.code() as u32);
                    self.security = Some(t);
                    self.begin_security(t);
                } else {
                    let offer = self.cfg.offerable();
                    if offer.is_empty() {
                        let reason = b"no security type configured";
                        self.out.put_u8(0);
                        self.out.put_u32(reason.len() as u32);
                        self.out.extend_from_slice(reason);
                        return Err(HandshakeError::NoCommonSecurity { offered: self.cfg.security.clone() });
                    }
                    let n_types = u8::try_from(offer.len()).expect("more than 255 security types");
                    self.out.put_u8(n_types);
                    for t in &offer {
                        self.out.put_u8(t.code());
                    }
                    self.state = ServerState::AwaitSecurityChoice;
                }
                Ok(Some(n))
            }
            ServerState::AwaitSecurityChoice => {
                let mut r = Reader::new(input);
                let t = SecurityType::from_code(r.u8()?);
                if !self.cfg.offerable().contains(&t) {
                    self.fail_security("security type not offered");
                    return Err(HandshakeError::UnexpectedSecurity(t));
                }
                self.security = Some(t);
                self.begin_security(t);
                Ok(Some(r.consumed()))
            }
            ServerState::AwaitVncResponse => {
                let mut r = Reader::new(input);
                let response = r.bytes(CHALLENGE_LEN)?;
                let auth = self.cfg.vnc_auth.as_ref().expect("VncAuth only offered when configured");
                let expected = vnc_auth_response(&auth.password, &auth.challenge);
                if !constant_time_eq(response, &expected) {
                    self.fail_security("authentication failed");
                    return Err(HandshakeError::AuthFailed(None));
                }
                self.out.put_u32(0);
                self.state = ServerState::AwaitClientInit;
                Ok(Some(r.consumed()))
            }
            ServerState::AwaitVeNCryptVersion => {
                let mut r = Reader::new(input);
                let (major, minor) = (r.u8()?, r.u8()?);
                if (major, minor) != VENCRYPT_VERSION {
                    self.out.put_u8(0xff);
                    return Err(HandshakeError::VeNCryptVersion { major, minor });
                }
                let offer = self.cfg.offerable_subtypes();
                self.out.put_u8(0);
                self.out.put_u8(u8::try_from(offer.len()).expect("bounded by max_vencrypt_subtypes"));
                for t in &offer {
                    self.out.put_u32(t.code());
                }
                self.state = ServerState::AwaitVeNCryptSubtype;
                Ok(Some(r.consumed()))
            }
            ServerState::AwaitVeNCryptSubtype => {
                let mut r = Reader::new(input);
                let t = VeNCryptSubtype::from_code(r.u32()?);
                if !self.cfg.offerable_subtypes().contains(&t) {
                    self.out.put_u8(0);
                    return Err(HandshakeError::UnexpectedSubtype(t));
                }
                self.out.put_u8(1);
                self.subtype = Some(t);
                self.state = ServerState::AwaitTls(t);
                Ok(Some(r.consumed()))
            }
            // Everything after the ack belongs to TLS; consume nothing.
            ServerState::AwaitTls(_) => Ok(None),
            ServerState::AwaitPlainCredentials => {
                match PlainCredentials::decode(input, self.cfg.limits.max_credential_len) {
                    Ok((creds, n)) => {
                        self.state = ServerState::AwaitPlainVerdict(creds);
                        Ok(Some(n))
                    }
                    Err(e @ DecodeError::Invalid(_)) => {
                        self.fail_security("credentials too long");
                        Err(e.into())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            // Waiting for the embedder to call `resolve_plain`.
            ServerState::AwaitPlainVerdict(_) => Ok(None),
            ServerState::AwaitClientInit => {
                let (ci, n) = ClientInit::decode(input)?;
                self.cfg.server_init.encode(&mut self.out);
                self.state = ServerState::Done(ci);
                Ok(Some(n))
            }
            ServerState::Done(_) => Ok(None),
            ServerState::Failed => Err(HandshakeError::Finished),
        }
    }
}
