//! Minimal blocking probe: connects to a VNC server, runs the handshake with
//! rfb-proto, and prints what was negotiated.
//! Usage: cargo run -p rfb-proto --example probe -- HOST:PORT [PASSWORD]
//! (A password on the command line is for local testing only.)
//! rfb-proto has no TLS, so this probe speaks unencrypted RFB only; for
//! VeNCrypt with TLS 1.3 use `cargo run -p rfb-tls --example probe`.

use std::io::{Read, Write};
use std::net::TcpStream;

use rfb_proto::{ClientConfig, ClientHandshake, SecurityType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().ok_or("usage: probe HOST:PORT [PASSWORD]")?;
    let password = args.next().map(String::into_bytes);
    let mut cfg = ClientConfig { security: vec![SecurityType::None], ..Default::default() };
    if password.is_some() {
        cfg.security.insert(0, SecurityType::VncAuth);
        cfg.password = password;
    }
    let mut sock = TcpStream::connect(&addr)?;
    let mut hs = ClientHandshake::new(cfg);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !hs.is_done() {
        let n = sock.read(&mut chunk)?;
        if n == 0 {
            return Err("server closed the connection".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        let used = hs.receive(&buf)?;
        buf.drain(..used);
        sock.write_all(&hs.take_output())?;
    }
    let si = hs.server_init().unwrap();
    println!(
        "OK version={:?} security={:?} size={}x{} name={:?}",
        hs.version().unwrap(),
        hs.security_type().unwrap(),
        si.width,
        si.height,
        String::from_utf8_lossy(&si.name)
    );
    Ok(())
}
