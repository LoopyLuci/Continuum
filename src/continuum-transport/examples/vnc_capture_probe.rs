// Live check: VncCaptureBackend against the running QEMU guest.
//
// Not a unit test -- it needs the guest, so it is an example.
//   cargo run -p continuum-transport --example vnc_capture_probe

use std::time::{Duration, Instant};

use continuum_core::capture::CaptureBackend as _;
use continuum_transport::{vnc_port, VncCaptureBackend};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let display: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], vnc_port(display)));
    println!("connecting to {addr} (display {display})");

    let started = Instant::now();
    let mut backend = VncCaptureBackend::connect("omarchy", addr, 80, 1280, 800)?;
    println!("connected in {:?}", started.elapsed());

    println!("monitors: {:?}", backend.enumerate_monitors()?);

    // First frame: the reader thread has to wait for an update to arrive.
    let mut frames = 0u32;
    let mut first_latency = Duration::ZERO;
    for i in 0..8 {
        let t0 = Instant::now();
        match backend.capture_frame(0) {
            Ok(frame) => {
                if frames == 0 {
                    first_latency = t0.elapsed();
                    println!(
                        "frame 0: {}x{} {} bytes in {:?}",
                        frame.width,
                        frame.height,
                        frame.data.len(),
                        first_latency
                    );
                }
                frames += 1;
            }
            Err(e) => {
                println!("frame {i}: {e}");
                break;
            }
        }
        if i == 0 {
            println!("subsequent frames are taken from the slot without blocking");
        }
    }
    println!("captured {frames} frames");
    Ok(())
}
