//! `vnc-client`: command-line front end for the Aurora VNC viewer core.
//!
//! ```text
//! vnc-client probe       HOST:PORT           handshake and print what was negotiated
//! vnc-client snapshot    HOST:PORT OUT.ppm   save one full frame as a PPM image
//! vnc-client fingerprint HOST:PORT           print the server's TLS certificate fingerprint
//! ```
//!
//! Connections are encrypted (VeNCrypt + TLS 1.3) by default and the server
//! must be authenticated, by a pinned fingerprint or a CA. Secrets come from
//! the environment so they never show up in shell history or `ps`.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Instant;

use rfb_proto::VeNCryptSubtype;
use vnc_client::config::{options, timeout, tls_options, ENV_HELP};
use vnc_client::net::{Connection, NetError};
use vnc_client::rfb_tls::{format_fingerprint, ServerVerification};
use vnc_client::{Event, SessionOptions};

const USAGE: &str = "usage:
  vnc-client probe       HOST:PORT
  vnc-client snapshot    HOST:PORT OUT.ppm
  vnc-client fingerprint HOST:PORT";

fn describe_tls(c: &Connection) -> String {
    match c.session.tls_info() {
        Some(i) => format!(
            "{:?} {:?} cert={}",
            i.protocol,
            i.cipher_suite,
            i.peer_certificate_sha256.as_ref().map(format_fingerprint).unwrap_or_default()
        ),
        None => "none".into(),
    }
}

/// Completes only the TLS part of VeNCrypt with an unverified server and
/// prints its certificate fingerprint. It stops before any RFB authentication,
/// so no password or username is ever sent.
fn fingerprint(addr: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut opts = SessionOptions::encrypted(tls_options(addr, ServerVerification::InsecureAcceptAny)?);
    // Subtypes that send nothing secret right after TLS. The dummy password
    // only makes X509Vnc selectable; we disconnect before answering its challenge.
    opts.handshake.vencrypt_subtypes = vec![VeNCryptSubtype::X509None, VeNCryptSubtype::X509Vnc];
    opts.handshake.password = Some(Vec::new());
    opts.request_initial_frame = false;
    let mut c = Connection::open(addr, opts, timeout())?;
    let deadline = Instant::now() + timeout();
    loop {
        if let Some(fp) = c.session.tls_info().and_then(|i| i.peer_certificate_sha256) {
            println!("{}", format_fingerprint(&fp));
            return Ok(());
        }
        match c.pump_once(deadline) {
            Ok(_) => {}
            Err(NetError::Client(e)) => return Err(format!("server did not offer certificate-based TLS: {e}").into()),
            Err(e) => return Err(e.into()),
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match args {
        [cmd, addr] if cmd == "fingerprint" => fingerprint(addr),
        [cmd, addr] if cmd == "probe" => {
            let mut opts = options(addr)?;
            opts.request_initial_frame = false;
            let c = Connection::connect(addr.as_str(), opts, timeout())?;
            let si = c.session.server_init().ok_or("no ServerInit")?;
            println!(
                "connected: {}x{} name={:?} tls={} server-pixel-format={:?}",
                si.width,
                si.height,
                String::from_utf8_lossy(&si.name),
                describe_tls(&c),
                si.pixel_format
            );
            Ok(())
        }
        [cmd, addr, out] if cmd == "snapshot" => {
            let mut opts = options(addr)?;
            opts.continuous_updates = false;
            let mut c = Connection::connect(addr.as_str(), opts, timeout())?;
            // Wait for real pixels: some servers (TightVNC) first send an
            // update with only the size and cursor, which would save black.
            let deadline = std::time::Instant::now() + timeout();
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                c.wait_for(left, |e| matches!(e, Event::Updated { damage } if !damage.is_empty()))?;
                if c.session.has_pixels() {
                    break;
                }
            }
            let fb = c.session.framebuffer();
            std::fs::write(out, fb.to_ppm())?;
            println!("saved {}x{} frame to {out} (tls={})", fb.width(), fb.height(), describe_tls(&c));
            Ok(())
        }
        _ => Err(format!("{USAGE}\n{ENV_HELP}").into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vnc-client: {e}");
            ExitCode::FAILURE
        }
    }
}
