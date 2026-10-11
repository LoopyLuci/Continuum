//! Blocking probe with VeNCrypt: connects, negotiates TLS 1.3 when the server
//! offers VeNCrypt, and prints what was negotiated.
//!
//! Usage: cargo run -p rfb-tls --example probe -- HOST:PORT
//!            [--pin SHA256 | --ca FILE | --insecure] [--user NAME] [--allow-plain]
//!
//! The password comes from `AURORA_VNC_PASSWORD`, never the command line.
//! Without `--allow-plain` only VeNCrypt is accepted (encrypted by default).

use std::net::TcpStream;
use std::time::Duration;

use rfb_proto::{ClientConfig, ClientHandshake, SecurityType};
use rfb_tls::{client_config, connect, format_fingerprint, parse_fingerprint, ClientTls, ServerVerification};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: probe HOST:PORT [--pin SHA256 | --ca FILE | --insecure] [--user NAME] [--allow-plain]";
    let addr = args.next().ok_or(usage)?;
    let mut verification = None;
    let mut cfg = ClientConfig::encrypted();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pin" => {
                verification = Some(ServerVerification::PinnedSha256(parse_fingerprint(&args.next().ok_or(usage)?)?))
            }
            "--ca" => verification = Some(ServerVerification::ca_file(args.next().ok_or(usage)?)?),
            "--insecure" => verification = Some(ServerVerification::InsecureAcceptAny),
            "--user" => cfg.username = Some(args.next().ok_or(usage)?.into_bytes()),
            "--allow-plain" => cfg.security.extend([SecurityType::VncAuth, SecurityType::None]),
            _ => return Err(usage.into()),
        }
    }
    cfg.password = std::env::var("AURORA_VNC_PASSWORD").ok().map(String::into_bytes);
    let verification = verification.ok_or("choose how to trust the server: --pin, --ca, or --insecure")?;
    let host = addr.rsplit_once(':').map_or(addr.as_str(), |(h, _)| h).trim_matches(['[', ']']);
    let tls = ClientTls { config: client_config(verification)?, server_name: host.to_owned() };

    let sock = TcpStream::connect(&addr)?;
    sock.set_read_timeout(Some(Duration::from_secs(20)))?;
    sock.set_write_timeout(Some(Duration::from_secs(20)))?;
    let c = connect(sock, ClientHandshake::new(cfg), Some(&tls))?;
    let hs = &c.handshake;
    let si = hs.server_init().expect("handshake done");
    println!(
        "OK version={:?} security={:?} subtype={:?} size={}x{} name={:?}",
        hs.version().unwrap(),
        hs.security_type().unwrap(),
        hs.vencrypt_subtype(),
        si.width,
        si.height,
        String::from_utf8_lossy(&si.name)
    );
    match c.stream.tls_info() {
        Some(t) => println!(
            "TLS protocol={:?} suite={:?} server_cert_sha256={}",
            t.protocol,
            t.cipher_suite,
            t.peer_certificate_sha256.as_ref().map(format_fingerprint).unwrap_or_default()
        ),
        None => println!("WARNING: connection is NOT encrypted"),
    }
    Ok(())
}
