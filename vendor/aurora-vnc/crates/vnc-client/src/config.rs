//! Connection settings from the environment, shared by every Aurora front end
//! (`vnc-client`, `vnc-viewer`). Secrets come from the environment so they
//! never show up in shell history or `ps`.

use std::time::Duration;

use rfb_proto::SecurityType;
use rfb_tls::{client_config, parse_fingerprint, ServerVerification};

use crate::{SessionOptions, TlsOptions};

/// Help text for the variables read here, for front ends' usage messages.
pub const ENV_HELP: &str = "environment:
  AURORA_VNC_TLS        how to trust the server (default: required, no trust configured)
                          pin:SHA256-HEX   accept exactly this certificate
                          ca:PATH          verify against these CA certificates (PEM)
                          insecure         encrypt but accept any certificate (testing only)
                          off              no encryption (legacy servers on trusted networks only)
  AURORA_VNC_TLS_NAME   name the certificate must match (default: the HOST part)
  AURORA_VNC_PASSWORD   VNC password, if the server needs one
  AURORA_VNC_USERNAME   username, for servers using VeNCrypt Plain auth
  AURORA_VNC_TIMEOUT    seconds to wait (default 15)
  AURORA_VNC_QUALITY    lossless (default), or 0-9 to let the server send JPEG at that quality
  AURORA_VNC_COMPRESS   0-9, how hard the server should compress (default: the server's choice)
  AURORA_VNC_RESIZE     viewer only: remote (default) resizes the remote desktop to the window
                          when the server allows it; scale keeps its size and scales the picture
  AURORA_VNC_CLIPBOARD  viewer only: on (default) shares the clipboard both ways (UTF-8 with
                          Extended Clipboard, Latin-1 otherwise); off shares nothing";

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn host_of(addr: &str) -> String {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    host.trim_start_matches('[').trim_end_matches(']').to_string()
}

pub fn tls_options(addr: &str, verification: ServerVerification) -> Result<TlsOptions, BoxError> {
    Ok(TlsOptions {
        config: client_config(verification)?,
        server_name: env("AURORA_VNC_TLS_NAME").unwrap_or_else(|| host_of(addr)),
    })
}

/// Builds session options for `addr` from the `AURORA_VNC_*` variables.
/// Refuses (with advice) when encryption is on but no server trust is set.
pub fn options(addr: &str) -> Result<SessionOptions, BoxError> {
    let mode = env("AURORA_VNC_TLS");
    let mut opts = match mode.as_deref() {
        Some("off") => {
            eprintln!("aurora-vnc: warning: encryption is OFF; everything, including the screen, is sent in the clear");
            SessionOptions::default()
        }
        Some("insecure") => {
            eprintln!("aurora-vnc: warning: the server's identity is NOT verified");
            SessionOptions::encrypted(tls_options(addr, ServerVerification::InsecureAcceptAny)?)
        }
        Some(m) if m.starts_with("pin:") => {
            let fp = parse_fingerprint(&m[4..])?;
            SessionOptions::encrypted(tls_options(addr, ServerVerification::PinnedSha256(fp))?)
        }
        Some(m) if m.starts_with("ca:") => {
            SessionOptions::encrypted(tls_options(addr, ServerVerification::ca_file(&m[3..])?)?)
        }
        Some(other) => return Err(format!("unknown AURORA_VNC_TLS value {other:?}\n{ENV_HELP}").into()),
        None => {
            return Err(format!(
                "connections are encrypted and the server must be verified, but no trust is configured.\n\
                 Run `vnc-client fingerprint {addr}`, check the fingerprint with the server's owner, then set\n\
                 AURORA_VNC_TLS=pin:<fingerprint> (or AURORA_VNC_TLS=ca:<file>)."
            )
            .into())
        }
    };
    if let Some(pw) = env("AURORA_VNC_PASSWORD") {
        if !opts.handshake.security.contains(&SecurityType::VeNCrypt) {
            opts.handshake.security.insert(0, SecurityType::VncAuth);
        }
        opts.handshake.password = Some(pw.into_bytes());
    }
    opts.handshake.username = env("AURORA_VNC_USERNAME").map(String::into_bytes);
    encoding_prefs(&mut opts, env("AURORA_VNC_QUALITY").as_deref(), env("AURORA_VNC_COMPRESS").as_deref())?;
    Ok(opts)
}

/// Applies the picture-quality and compression settings by adding the
/// matching pseudo-encodings. JPEG is lossy, so it is only ever requested
/// when a quality level is given; `lossless` (or nothing) never asks for it.
pub fn encoding_prefs(
    opts: &mut SessionOptions,
    quality: Option<&str>,
    compress: Option<&str>,
) -> Result<(), BoxError> {
    let level = |name: &str, v: &str| -> Result<u8, BoxError> {
        match v.parse::<u8>() {
            Ok(l) if l <= 9 => Ok(l),
            _ => Err(format!("{name} must be 0-9, got {v:?}").into()),
        }
    };
    match quality {
        None | Some("lossless") => {}
        Some(q) => opts.encodings.push(crate::jpeg_quality(level("AURORA_VNC_QUALITY", q)?)),
    }
    if let Some(c) = compress {
        opts.encodings.push(crate::compress_level(level("AURORA_VNC_COMPRESS", c)?));
    }
    Ok(())
}

pub fn timeout() -> Duration {
    Duration::from_secs(env("AURORA_VNC_TIMEOUT").and_then(|s| s.parse().ok()).unwrap_or(15))
}

#[cfg(test)]
mod prefs_tests {
    use super::*;
    use rfb_proto::Encoding;

    #[test]
    fn lossless_by_default() {
        let mut o = SessionOptions::default();
        let before = o.encodings.clone();
        encoding_prefs(&mut o, None, None).unwrap();
        encoding_prefs(&mut o, Some("lossless"), None).unwrap();
        assert_eq!(o.encodings, before);
    }

    #[test]
    fn quality_and_compress_add_pseudo_encodings() {
        let mut o = SessionOptions::default();
        encoding_prefs(&mut o, Some("7"), Some("2")).unwrap();
        assert!(o.encodings.ends_with(&[Encoding::Other(-25), Encoding::Other(-254)]));
    }

    #[test]
    fn bad_levels_are_errors() {
        for (q, c) in [(Some("10"), None), (Some("high"), None), (None, Some("-1"))] {
            assert!(encoding_prefs(&mut SessionOptions::default(), q, c).is_err());
        }
    }
}
