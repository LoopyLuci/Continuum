use anyhow::{Context, Result};
use base64::Engine;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CERT_DIR: &str = "continuum";

pub struct TlsCert {
    pub cert_der: CertificateDer<'static>,
    pub key_der: PrivatePkcs8KeyDer<'static>,
    pub fingerprint: String,
}

pub fn generate_self_signed() -> Result<TlsCert> {
    let key_pair = KeyPair::generate()?;
    let params = CertificateParams::new(vec!["localhost".into(), "continuum.local".into()])?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der: CertificateDer<'static> = cert.der().as_ref().to_vec().into();
    let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

    let fingerprint = {
        let mut hasher = Sha256::new();
        hasher.update(cert_der.as_ref());
        let hash = hasher.finalize();
        base64::engine::general_purpose::STANDARD.encode(hash)
    };

    Ok(TlsCert {
        cert_der,
        key_der,
        fingerprint,
    })
}

pub fn load_or_generate_cert(cert_path: Option<&Path>, key_path: Option<&Path>) -> Result<TlsCert> {
    if let (Some(cert_p), Some(key_p)) = (cert_path, key_path) {
        if cert_p.exists() && key_p.exists() {
            return load_cert_from_files(cert_p, key_p);
        }
    }

    let data_dir = dirs::data_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let cert_dir = data_dir.join(CERT_DIR);
    std::fs::create_dir_all(&cert_dir)?;

    let cert_file = cert_dir.join("cert.der");
    let key_file = cert_dir.join("key.der");
    let fp_file = cert_dir.join("fingerprint.txt");

    if cert_file.exists() && key_file.exists() {
        match load_cert_from_files(&cert_file, &key_file) {
            Ok(cert) => {
                tracing::info!(fingerprint = %cert.fingerprint, "Loaded persisted TLS certificate");
                return Ok(cert);
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load persisted cert, regenerating");
            }
        }
    }

    let cert = generate_self_signed()?;

    std::fs::write(&cert_file, cert.cert_der.as_ref())?;
    std::fs::write(&key_file, cert.key_der.secret_pkcs8_der())?;
    std::fs::write(&fp_file, &cert.fingerprint)?;

    tracing::info!(
        fingerprint = %cert.fingerprint,
        cert_dir = %cert_dir.display(),
        "Generated and persisted new TLS certificate"
    );

    Ok(cert)
}

fn load_cert_from_files(cert_path: &Path, key_path: &Path) -> Result<TlsCert> {
    let cert_bytes = std::fs::read(cert_path)
        .with_context(|| format!("Failed to read cert: {}", cert_path.display()))?;
    let key_bytes = std::fs::read(key_path)
        .with_context(|| format!("Failed to read key: {}", key_path.display()))?;

    let cert_der: CertificateDer<'static> = cert_bytes.into();
    let key_der = PrivatePkcs8KeyDer::from(key_bytes);

    let fingerprint = {
        let mut hasher = Sha256::new();
        hasher.update(cert_der.as_ref());
        let hash = hasher.finalize();
        base64::engine::general_purpose::STANDARD.encode(hash)
    };

    Ok(TlsCert {
        cert_der,
        key_der,
        fingerprint,
    })
}

pub fn build_server_config(cert: TlsCert) -> Result<rustls::ServerConfig> {
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert_der], cert.key_der.into())?;
    config.alpn_protocols = vec![crate::types::ALPN.to_vec()];
    Ok(config)
}

pub fn build_client_config(pinned_fingerprint: Option<&str>) -> Result<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    config.alpn_protocols = vec![crate::types::ALPN.to_vec()];

    if let Some(fingerprint) = pinned_fingerprint {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(PinnedCertVerifier {
                expected_fingerprint: fingerprint.to_string(),
            }));
    } else {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(TofuVerifier::new()));
    }

    Ok(config)
}

#[derive(Debug)]
struct TofuVerifier {
    pinned: parking_lot::RwLock<Option<String>>,
}

impl TofuVerifier {
    fn new() -> Self {
        let pinned = load_pinned_fingerprint();
        Self {
            pinned: parking_lot::RwLock::new(pinned),
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let hash = hasher.finalize();
        let fingerprint = base64::engine::general_purpose::STANDARD.encode(hash);

        let mut pinned = self.pinned.write();
        match pinned.as_ref() {
            Some(known) => {
                if *known != fingerprint {
                    tracing::warn!(
                        expected = %known,
                        got = %fingerprint,
                        "Server certificate fingerprint mismatch — rejecting connection"
                    );
                    return Err(rustls::Error::General(
                        "Server certificate fingerprint changed. Reconnection refused for security.".to_string(),
                    ));
                }
            }
            None => {
                tracing::info!(fingerprint = %fingerprint, "Trusting server certificate (first use)");
                *pinned = Some(fingerprint.clone());
                save_pinned_fingerprint(&fingerprint);
            }
        }

        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        supported.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedCertVerifier {
    expected_fingerprint: String,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let hash = hasher.finalize();
        let fingerprint = base64::engine::general_purpose::STANDARD.encode(hash);

        if fingerprint != self.expected_fingerprint {
            tracing::error!(
                expected = %self.expected_fingerprint,
                got = %fingerprint,
                "Server certificate does not match pinned fingerprint"
            );
            return Err(rustls::Error::General(
                "Server certificate fingerprint mismatch".to_string(),
            ));
        }

        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        let supported = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        supported.supported_schemes()
    }
}

fn pinned_fingerprint_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    data_dir.join("continuum").join("server-fingerprint.txt")
}

fn load_pinned_fingerprint() -> Option<String> {
    std::fs::read_to_string(pinned_fingerprint_path())
        .ok()
        .map(|s| s.trim().to_string())
}

fn save_pinned_fingerprint(fingerprint: &str) {
    let path = pinned_fingerprint_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, fingerprint);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_self_signed() {
        let cert = generate_self_signed().unwrap();
        assert!(!cert.fingerprint.is_empty());
        assert!(!cert.cert_der.as_ref().is_empty());
    }

    #[test]
    fn test_generate_unique_certs() {
        let cert1 = generate_self_signed().unwrap();
        let cert2 = generate_self_signed().unwrap();
        assert_ne!(cert1.fingerprint, cert2.fingerprint);
    }

    #[test]
    fn test_build_server_config() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = generate_self_signed().unwrap();
        let config = build_server_config(cert);
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config_no_pin() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = build_client_config(None);
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config_with_pin() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = generate_self_signed().unwrap();
        let config = build_client_config(Some(&cert.fingerprint));
        assert!(config.is_ok());
    }

    #[test]
    fn test_tls_cert_fingerprint_is_base64() {
        let cert = generate_self_signed().unwrap();
        assert!(cert
            .fingerprint
            .chars()
            .all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='));
    }
}
