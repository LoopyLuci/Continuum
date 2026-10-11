//! Certificates: self-signed generation for first-run hosts, and SHA-256
//! fingerprints for pinning (trust on first use, or out-of-band checks).

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use rustls::pki_types::CertificateDer;

use crate::TlsError;

/// A freshly generated self-signed certificate and its private key.
#[derive(Clone)]
pub struct SelfSigned {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: CertificateDer<'static>,
    /// SHA-256 of the certificate DER, to show users or pin in clients.
    pub sha256: [u8; 32],
}

impl fmt::Debug for SelfSigned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelfSigned")
            .field("sha256", &format_fingerprint(&self.sha256))
            .field("key_pem", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl SelfSigned {
    /// Generates an ECDSA P-256 certificate valid for `names` (DNS names or
    /// IP addresses).
    pub fn generate(names: &[&str]) -> Result<Self, TlsError> {
        let names: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
        let ck = rcgen::generate_simple_self_signed(names).map_err(|e| TlsError::Certificate(e.to_string()))?;
        let cert_der = ck.cert.der().clone();
        Ok(SelfSigned {
            cert_pem: ck.cert.pem(),
            key_pem: ck.signing_key.serialize_pem(),
            sha256: fingerprint_sha256(&cert_der),
            cert_der,
        })
    }

    /// Loads `cert.pem` and `key.pem` from `dir`, generating and saving them
    /// first if either is missing. The key file is created with mode 0600 on
    /// Unix. This is what a host does on its first run.
    pub fn load_or_generate(dir: &Path, names: &[&str]) -> Result<Self, TlsError> {
        let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
        if cert_path.exists() && key_path.exists() {
            let cert_pem = fs::read_to_string(&cert_path)?;
            let key_pem = fs::read_to_string(&key_path)?;
            let cert_der = crate::config::parse_certs(cert_pem.as_bytes())?
                .into_iter()
                .next()
                .ok_or_else(|| TlsError::Certificate(format!("no certificate in {}", cert_path.display())))?;
            return Ok(SelfSigned { sha256: fingerprint_sha256(&cert_der), cert_pem, key_pem, cert_der });
        }
        let s = Self::generate(names)?;
        fs::create_dir_all(dir)?;
        write_private(&key_path, s.key_pem.as_bytes())?;
        fs::write(&cert_path, &s.cert_pem)?;
        Ok(s)
    }
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(data)
}

/// SHA-256 of a certificate's DER encoding.
pub fn fingerprint_sha256(cert_der: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, cert_der);
    d.as_ref().try_into().expect("SHA-256 is 32 bytes")
}

/// `AB:CD:...` form, as printed by `openssl x509 -fingerprint -sha256`.
pub fn format_fingerprint(fp: &[u8; 32]) -> String {
    fp.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Parses a SHA-256 fingerprint in hex, with or without `:` separators.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32], TlsError> {
    let hex: String = s.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect();
    let bad = || TlsError::Certificate(format!("not a SHA-256 fingerprint: {s:?}"));
    if hex.len() != 64 || !hex.is_ascii() {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}
