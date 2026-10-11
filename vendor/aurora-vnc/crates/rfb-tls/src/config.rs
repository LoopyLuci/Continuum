//! `rustls` configurations for both ends. Every config built here speaks
//! TLS 1.3 only, with the `ring` provider.

use std::fmt;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring as provider, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::cert::fingerprint_sha256;
use crate::TlsError;

/// How a client decides whether to trust the server's certificate.
#[derive(Clone)]
pub enum ServerVerification {
    /// Standard X.509 verification (chain, expiry, and server name) against
    /// these CA certificates in PEM. A host's own self-signed certificate
    /// works here too if it is not flagged as a CA (those from
    /// [`SelfSigned`](crate::SelfSigned) are not; `openssl req -x509` ones
    /// are, and are rejected as a leaf), otherwise pin it instead.
    CaPem(Vec<u8>),
    /// Standard X.509 verification against the operating system's CA store.
    #[cfg(feature = "system-roots")]
    SystemRoots,
    /// Accept exactly the certificate with this SHA-256 fingerprint, whatever
    /// its name or expiry: for self-signed hosts whose fingerprint the user
    /// confirmed once (trust on first use) or got out of band.
    PinnedSha256([u8; 32]),
    /// Accept any certificate. The connection is encrypted but the server is
    /// not authenticated. For tests and an explicit `--insecure` flag only.
    InsecureAcceptAny,
}

impl fmt::Debug for ServerVerification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerVerification::CaPem(p) => write!(f, "CaPem({} bytes)", p.len()),
            #[cfg(feature = "system-roots")]
            ServerVerification::SystemRoots => write!(f, "SystemRoots"),
            ServerVerification::PinnedSha256(fp) => {
                write!(f, "PinnedSha256({})", crate::cert::format_fingerprint(fp))
            }
            ServerVerification::InsecureAcceptAny => write!(f, "InsecureAcceptAny"),
        }
    }
}

impl ServerVerification {
    /// `CaPem` from a file.
    pub fn ca_file(path: impl AsRef<std::path::Path>) -> Result<Self, TlsError> {
        Ok(ServerVerification::CaPem(std::fs::read(path)?))
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(provider::default_provider())
}

/// Builds a TLS 1.3-only client configuration.
pub fn client_config(verification: ServerVerification) -> Result<Arc<rustls::ClientConfig>, TlsError> {
    let builder =
        rustls::ClientConfig::builder_with_provider(provider()).with_protocol_versions(&[&rustls::version::TLS13])?;
    let config = match verification {
        ServerVerification::CaPem(pem) => {
            let mut roots = RootCertStore::empty();
            for cert in parse_certs(&pem)? {
                roots.add(cert)?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        #[cfg(feature = "system-roots")]
        ServerVerification::SystemRoots => {
            let mut roots = RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            let (added, _ignored) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err(TlsError::Certificate("no usable CA certificates in the system store".into()));
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        ServerVerification::PinnedSha256(fp) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(FingerprintVerifier::new(Some(fp))))
            .with_no_client_auth(),
        ServerVerification::InsecureAcceptAny => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(FingerprintVerifier::new(None)))
            .with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// Builds a TLS 1.3-only server configuration from a PEM certificate chain
/// (leaf first) and a PEM private key (PKCS#8, PKCS#1, or SEC1).
pub fn server_config(cert_chain_pem: &[u8], key_pem: &[u8]) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let chain = parse_certs(cert_chain_pem)?;
    if chain.is_empty() {
        return Err(TlsError::Certificate("no certificate in PEM input".into()));
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| TlsError::Certificate(format!("private key: {e}")))?;
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    Ok(Arc::new(config))
}

pub(crate) fn parse_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsError::Certificate(format!("certificate PEM: {e}")))
}

/// Accepts the server certificate by fingerprint (or any, when `pin` is
/// `None`). The TLS 1.3 handshake signature is still verified, so the peer
/// must hold the private key of the certificate it presents.
#[derive(Debug)]
struct FingerprintVerifier {
    pin: Option<[u8; 32]>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl FingerprintVerifier {
    fn new(pin: Option<[u8; 32]>) -> Self {
        FingerprintVerifier { pin, algorithms: provider::default_provider().signature_verification_algorithms }
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            None => Ok(ServerCertVerified::assertion()),
            Some(pin) if rfb_proto::auth::constant_time_eq(&fingerprint_sha256(end_entity), &pin) => {
                Ok(ServerCertVerified::assertion())
            }
            Some(_) => Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // Unreachable: every config here is TLS 1.3 only.
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
