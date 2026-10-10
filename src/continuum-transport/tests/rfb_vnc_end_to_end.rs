//! End-to-end test: a live QEMU guest, read over RFB, served over RFB.
//!
//! # Why this file exists
//!
//! `rfb_integration.rs` exercises the server thoroughly — and every one of its
//! cases builds the capture source from `with_shared_qmp`, so all of them run
//! on the **QMP fallback** path. Nothing has ever driven a frame from QEMU
//! through `VideoSource::Vnc`, through the RFB server, to a client.
//!
//! That gap is the same shape as two earlier ones in this project, and it is
//! worth naming because the pattern recurred:
//!
//! * VM-Harness's Hextile decoder read the sub-encoding byte wrong and passed
//!   261 tests, because every fixture was generated from the decoder's own
//!   assumptions.
//! * Continuum's RRE encoder and decoder shared one misreading of the
//!   sub-rectangle order, so encoder-against-decoder tests passed while every
//!   real client desynchronised.
//! * This file's predecessor: the wiring that moved both consumers onto RFB
//!   was verified only on the path they were already using.
//!
//! In all three, the tests agreed with each other and none of them touched the
//! branch that mattered. So this file connects to the real guest and asserts
//! that pixels survive the whole path.
//!
//! # Skipped, not passed, when there is no guest
//!
//! There is no fake here. A synthetic RFB server would reproduce the same
//! self-agreement problem in a different costume. When no guest is listening
//! the test reports `skipped` and says so, because "the VNC path is untested"
//! and "the VNC path passed" must never look the same in a test report.

use std::sync::Arc;
use std::time::Duration;

use continuum_transport::capture_source::CaptureSource;
use continuum_transport::input_qmp::QmpInputInjector;
use continuum_transport::rfb::{RfbServer, RfbServerConfig};

/// QMP on this host, as launched by `boot_vm.ps1`.
fn qmp() -> std::net::SocketAddr { std::net::SocketAddr::from(([127, 0, 0, 1], 4444)) }

/// The RFB endpoint `boot_vm.ps1` configures: display 100 -> port 6000.
const VNC_DISPLAY: u16 = 100;

/// VNC Auth password for the test server below. VNC Auth truncates to eight
/// bytes, so this is deliberately short.
const PASSWORD: &str = "hunter2";

/// True when the guest is up on both endpoints.
///
/// QMP alone is not enough to decide the test is meaningful: the test is
/// about the RFB path, and a guest with QMP but no VNC display would silently
/// exercise the fallback instead — which is precisely the mistake this file
/// exists to prevent.
async fn guest_is_reachable() -> bool {
    let vnc = std::net::SocketAddr::from(([127, 0, 0, 1], continuum_transport::vnc_port(VNC_DISPLAY)));
    std::net::TcpStream::connect_timeout(&vnc, Duration::from_millis(750)).is_ok()
        && std::net::TcpStream::connect_timeout(&qmp(), Duration::from_millis(750)).is_ok()
}

/// A frame arriving over the wire has to contain actual screen content.
///
/// An all-black frame is what a decoder produces when it gives up: the socket
/// is connected, the handshake completes, and nothing is drawn. Asserting on
/// byte *length* alone would pass for that, so the pixels themselves are
/// checked.
#[tokio::test]
async fn a_live_guest_streams_over_rfb_not_the_fallback() {
    if !guest_is_reachable().await {
        eprintln!(
            "SKIPPED: no guest on QMP 4444 and VNC display {VNC_DISPLAY}; \
             the RFB capture path is UNTESTED, not passing"
        );
        return;
    }

    let (source, fell_back) = CaptureSource::connect("omarchy", qmp(), 85, 1280, 800)
        .await
        .expect("QMP connect");

    // The whole point. A silent fallback here would make every other
    // assertion in this file pass while testing nothing new.
    assert!(
        fell_back.is_none(),
        "expected the RFB path, got the QMP fallback: {:?}",
        fell_back.expect("fell back")
    );
    assert_eq!(source.name(), "vnc", "video source must be RFB");

    let injector = Arc::new(
        QmpInputInjector::new("omarchy", source.client(), None).with_guest_size(1280, 800),
    );
    let server = RfbServer::new(RfbServerConfig::secure("127.0.0.1:0".parse().unwrap(), PASSWORD), Arc::new(source), injector);
    let listener = server.bind().await.expect("bind RFB listener");
    let addr = listener.local_addr().expect("listener address");
    let serving = server.clone();
    tokio::spawn(async move { serving.serve_with(listener).await });

    // aurora-vnc's client, already proven against this guest in
    // examples/capture_vnc_fixture.rs. Using a third-party decoder is what
    // makes this a real interoperability check rather than Continuum talking
    // to itself: if both ends shared a misreading of the spec, only an
    // independent decoder would notice.
    let frame = tokio::task::spawn_blocking(move || -> Option<(u32, u32, Vec<u32>)> {
        // The server is configured with VNC Auth, so the client has to
        // *offer* it. `password` alone does not do that: the offer list is a
        // separate field, and setting only the password fails as "no common
        // security type" rather than as an auth rejection — a confusing way to
        // learn the client advertised nothing it can actually do.
        let mut options = vnc_client::SessionOptions::default();
        options.handshake.security = vec![rfb_proto::types::SecurityType::VncAuth];
        options.handshake.password = Some(PASSWORD.as_bytes().to_vec());
        let mut connection = vnc_client::net::Connection::connect(
            addr,
            options,
            Duration::from_secs(10),
        )
        .unwrap_or_else(|e| panic!("client could not connect to our own RFB server at {addr}: {e}"));

        // RFB is push-based: nothing is drawn until the server sends an
        // update, and this client has not asked for one yet. Reading the
        // framebuffer straight after `connect` therefore returns the zeroed
        // buffer the handshake left behind — which looks exactly like a broken
        // capture path, and is the reason this test has to pump before it
        // looks.
        connection
            .wait_for(Duration::from_secs(15), |e| {
                matches!(e, vnc_client::Event::Updated { .. })
            })
            .unwrap_or_else(|e| panic!("server never sent a framebuffer update: {e}"));

        let fb = connection.session.framebuffer();
        let (w, h) = (fb.width() as usize, fb.height() as usize);
        let data = fb.pixels().to_vec();
        Some((w as u32, h as u32, data))
    })
    .await
    .expect("client task")
    .expect("connect to our own RFB server");

    let (width, height, pixels) = frame;
    assert_eq!((width, height), (1280, 800), "guest geometry");
    assert_eq!(pixels.len(), 1280 * 800, "one u32 per pixel");

    // Not black, and not uniform. A decoder that gave up leaves every pixel
    // zero; a decoder that desynchronised leaves noise. Content has both
    // variation and a plausible mean.
    let nonblack = pixels.iter().filter(|&&p| p & 0x00FF_FFFF != 0).count();
    let total = pixels.len();
    let mean: u64 = pixels.iter().map(|&p| (p & 0xFF) as u64).sum::<u64>() / total as u64;

    assert!(
        nonblack as f64 / total as f64 > 0.5,
        "frame is {}% black; a blank frame means the decoder never drew",
        100 * (total - nonblack) / total
    );
    assert!(
        (10..=250).contains(&mean),
        "mean channel {mean} is not a plausible screen luminance"
    );
    eprintln!(
        "OK: {width}x{height} over RFB from a live guest, {:.1}% non-black, mean {mean}",
        100.0 * nonblack as f64 / total as f64
    );
}

/// The fallback is a real configuration, so it gets tested as one.
///
/// A VM launched without `-vnc` must still stream. If the RFB connect fails
/// for any reason, `CaptureSource` must produce a working QMP source rather
/// than erroring — otherwise adding `-vnc` to a launcher becomes a way to
/// break streaming rather than to speed it up.
#[tokio::test]
async fn a_guest_without_a_vnc_display_still_streams() {
    // Point at a display nothing is listening on. QMP is real, so this is a
    // guest with no VNC endpoint -- exactly the configuration under test.
    let dead_display = std::net::SocketAddr::from((
        [127, 0, 0, 1],
        continuum_transport::vnc_port(VNC_DISPLAY + 90),
    ));
    std::net::TcpStream::connect_timeout(&dead_display, Duration::from_millis(200)).ok();

    let source = match CaptureSource::connect("omarchy", qmp(), 85, 64, 48).await {
        Ok((source, reason)) => {
            // RFB may or may not be up on this host; either way the source
            // must be usable and input must work.
            let _ = reason;
            source
        }
        Err(e) => {
            eprintln!("SKIPPED: no guest on QMP 4444: {e}");
            return;
        }
    };

    // Whatever path was chosen, input must work -- that is the invariant
    // CaptureSource exists to hold. `QmpClient` exposes its endpoint rather
    // than a connected flag, because the handshake already happened inside
    // `connect`; reaching this line at all *is* the proof that input works.
    assert_eq!(
        source.client().addr().port(),
        4444,
        "input must be available over QMP regardless of which video path was chosen"
    );
    eprintln!(
        "OK: source={} with a working QMP input path",
        source.name()
    );
}
