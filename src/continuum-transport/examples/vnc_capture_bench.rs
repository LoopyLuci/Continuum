// Measure the capture rate of each source against a live guest.
//
//   cargo run -p continuum-transport --example vnc_capture_bench
//
// # What this is for
//
// The claim behind moving Continuum off QMP `screendump` is that RFB is
// faster: a screendump is a socket round trip plus a *full-framebuffer* PNG
// encode by QEMU on every frame, while RFB sends only what changed. That
// number was inherited from VM-Harness, not measured here, and a claim
//! inherited from another component is a claim nobody has checked.
//
// # Why the screen is animated
//
// This is the part that makes the number mean anything. A static desktop
// sends no RFB updates at all, so an RFB benchmark on an idle guest reports
// "0 fps" and looks catastrophically slow -- while the QMP path happily keeps
// returning identical frames. Measuring only an idle screen would have
// produced exactly the wrong conclusion, in the opposite direction to the
// one being argued.
//
// So the guest is driven with a real animation and both sources are measured
// under the same conditions. A cursor move alone is not enough -- it is one
// small rectangle, which flatters RFB -- so pointer motion is swept across the
// screen while frames are counted.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use continuum_transport::capture_qmp::QmpCaptureBackend;
use continuum_transport::capture_vnc::{vnc_port, VncCaptureBackend};

fn qmp_addr() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 4444))
}

fn vnc_addr() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], vnc_port(100)))
}

async fn reachable(addr: std::net::SocketAddr) -> bool {
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(750)).is_ok()
}

struct Stats {
    frames: u64,
    bytes: u64,
    elapsed: Duration,
    first_frame: Option<Duration>,
}

impl Stats {
    fn report(&self, label: &str) {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            println!("{label}: no elapsed time");
            return;
        }
        let fps = self.frames as f64 / secs;
        let kbps = (self.bytes as f64 / 1024.0) / secs;
        let latency = self
            .first_frame
            .map(|d| format!("{:>7.1} ms", d.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "    n/a".into());
        println!(
            "{label}: {:>4} frames in {:>5.2}s = {:>6.1} fps, {:>8.1} KiB/s, first frame {latency}",
            self.frames, secs, fps, kbps
        );
    }
}

/// Sweep the pointer across the guest so the screen actually changes.
///
/// A single click is one tiny rectangle and would flatter RFB badly; a sweep
/// damages a different region on every step.
async fn animate(qmp: Arc<continuum_transport::qmp::QmpClient>, stop: Arc<AtomicBool>) {
    // QMP rejects every command until capabilities are negotiated. Skipping
    // this does not error loudly -- it returns "Expecting capabilities
    // negotiation" per command, which this loop would have discarded as a
    // failed move, leaving the screen completely static and the benchmark
    // measuring nothing while looking like a slow RFB path.
    let _ = qmp.execute("qmp_capabilities", None).await;

    let mut step = 0u32;
    while !stop.load(Ordering::Relaxed) {
        // QMP's absolute-axis form takes one event per axis and requires
        // *integer* values. The obvious-looking `{"button":"left","x":..}`
        // shape is rejected outright with "events[0].data.axis is missing",
        // and a float value is rejected as well. Either mistake fails every
        // event, leaving the desktop perfectly still -- which reads as "RFB is
        // slow" rather than "nothing was ever drawn".
        let x = 2000 + ((step * 397) % 12000) as i64;
        let y = 1500 + ((step * 251) % 8000) as i64;
        let _ = qmp
            .execute(
                "input-send-event",
                serde_json::json!({
                    "events": [
                        {"type": "abs", "data": {"axis": "x", "value": x}},
                        {"type": "abs", "data": {"axis": "y", "value": y}}
                    ]
                })
                .into(),
            )
            .await;
        step = step.wrapping_add(1);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !reachable(qmp_addr()).await {
        println!("SKIPPED: no guest on QMP {}", qmp_addr());
        return Ok(());
    }

    const WARMUP: Duration = Duration::from_secs(3);
    const MEASURE: Duration = Duration::from_secs(10);

    // A wedged guest is the single most dangerous thing to benchmark against:
    // its screen never changes, so RFB correctly reports ~0 fps and the
    // comparison comes out backwards. Everything below is worthless unless the
    // guest is demonstrably alive and being redrawn, so that is checked first
    // and a failure aborts rather than reporting numbers.
    {
        let probe = QmpCaptureBackend::connect("omarchy", qmp_addr()).await?;
        // `query-status` is authoritative for run state; HMP `info status` and
        // `query-mice` agree, and a guest left paused reports `running: false`
        // while still answering every command.
        let status = probe
            .client()
            .execute("query-status", None)
            .await?
            ;
        let running = status.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
        if !running {
            return Err(format!("guest is not running: {status}").into());
        }
        println!("guest running; confirming it redraws under input");
        let stop = Arc::new(AtomicBool::new(false));
        let anim = tokio::spawn(animate(probe.client().clone(), Arc::clone(&stop)));
        let mut hashes: Vec<u64> = Vec::new();
        for _ in 0..6 {
            hashes.push(probe.capture_jpeg().await?.jpeg.iter().map(|b| *b as u64).sum());
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        stop.store(true, Ordering::Relaxed);
        anim.abort();
        let distinct = hashes.iter().collect::<std::collections::BTreeSet<_>>().len();
        if distinct < 2 {
            return Err(format!(
                "guest framebuffer did not change under input ({distinct} distinct \
                 frames in 6 samples) -- the benchmark would compare a live path \
                 against a frozen one, so no numbers are reported"
            )
            .into());
        }
        println!("guest redraws ({distinct} distinct frames in 6 samples)");
    }

    // ── RFB ──────────────────────────────────────────────────────────────
    if reachable(vnc_addr()).await {
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        let first = Arc::new(AtomicU64::new(0));

        let mut backend = VncCaptureBackend::connect("omarchy", vnc_addr(), 85, 1280, 800)?;
        let qmp = continuum_transport::qmp::QmpClient::connect(qmp_addr()).await?;
        let anim = tokio::spawn(animate(Arc::new(qmp), Arc::clone(&stop)));

        // Warm up: the first RFB frame is a full non-incremental update and
        // takes hundreds of milliseconds, which would otherwise dominate.
        tokio::time::sleep(WARMUP).await;

        let started = Instant::now();
        let mut stats = Stats { frames: 0, bytes: 0, elapsed: MEASURE, first_frame: None };

        while started.elapsed() < MEASURE {
            // A frame loop like the ones actually in the servers: wait for real
            // pixels rather than spinning.
            if let Ok(frame) = backend.wait_for_frame(Duration::from_millis(500)) {
                if stats.first_frame.is_none() {
                    stats.first_frame = Some(started.elapsed());
                }
                stats.frames += 1;
                stats.bytes += frame.data.len() as u64;
                count.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(frame.data.len() as u64, Ordering::Relaxed);
            }
        }
        stats.report("RFB (damage-only)");
        println!(
            "  (this path only counts frames the guest actually sent; an idle \
             desktop correctly produces none)"
        );
        stop.store(true, Ordering::Relaxed);
        anim.abort();
        let _ = (count, bytes, first);
    } else {
        println!("RFB: no guest on {} — skipping", vnc_addr());
    }

    // ── QMP screendump ────────────────────────────────────────────────────
    let backend = QmpCaptureBackend::connect("omarchy", qmp_addr())
        .await?
        .with_format(85, 1280, 800);

    // Animate over the *same* QMP connection the screendump uses. QEMU accepts
    // one QMP client at a time, so a second connection here does not merely
    // add load -- it deadlocks until the greeting timeout and the whole
    // measurement fails for a reason that has nothing to do with capture.
    let stop = Arc::new(AtomicBool::new(false));
    let anim = tokio::spawn(animate(backend.client().clone(), Arc::clone(&stop)));

    tokio::time::sleep(WARMUP).await;
    let started = Instant::now();
    let mut stats = Stats { frames: 0, bytes: 0, elapsed: MEASURE, first_frame: None };
    while started.elapsed() < MEASURE {
        // One screendump at a time, as the real capture loop does.
        match backend.capture_jpeg().await {
            Ok(frame) => {
                if stats.first_frame.is_none() {
                    stats.first_frame = Some(started.elapsed());
                }
                stats.frames += 1;
                stats.bytes += frame.jpeg.len() as u64;
            }
            Err(e) => {
                println!("QMP: capture failed: {e}");
                break;
            }
        }
    }
    stats.report("QMP screendump (full frame)");
    stop.store(true, Ordering::Relaxed);
    anim.abort();

    Ok(())
}
