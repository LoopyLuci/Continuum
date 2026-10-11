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

/// Serialises the tests in this binary, because QEMU's QMP endpoint accepts
/// **one client at a time**.
///
/// Without this the two tests race: one binds QMP and the other's connect is
/// refused with `os error 10061`, which looks exactly like "the guest is not
/// running" and reports as a failure. It also made the results depend on
/// scheduling -- the pair passed reliably with `--test-threads=1` and failed in
/// the full suite, which is the worst kind of flake because it looks like the
/// guest is unstable.
///
/// A skip is only honest if it means the guest is absent, so the tests take a
/// turn rather than tolerating a refusal.
static QMP_SLOT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A frame arriving over the wire has to contain actual screen content.
///
/// An all-black frame is what a decoder produces when it gives up: the socket
/// is connected, the handshake completes, and nothing is drawn. Asserting on
/// byte *length* alone would pass for that, so the pixels themselves are
/// checked.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_guest_streams_over_rfb_not_the_fallback() {
    let _slot = QMP_SLOT.lock().await;
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

    // Captured before `source` is moved into the server below.
    let qmp_client = source.client();
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

    // Compare against ground truth from the *other* capture path rather than
    // against an assumption about what the guest should be showing.
    //
    // The earlier version of this test asserted the frame was not black, on the
    // reasoning that a decoder which gives up leaves every pixel zero. That
    // reasoning silently assumed the guest runs a graphical session. It does
    // not: booted headless, Omarchy sits at a serial login prompt with a black
    // framebuffer, and the test failed on a perfectly correct capture -- which
    // is a test that asserts the environment rather than the code.
    //
    // Asking QEMU what the screen actually contains fixes that. Both paths
    // read the same framebuffer at the same moment, so they must agree, and a
    // decoder that gave up now disagrees with QMP instead of with a hardcoded
    // expectation. It is also a stronger check: it validates the RFB path
    // against an independent source of truth rather than against "not black".
    let truth = qmp_screendump_mean(&qmp_client).await;
    let tolerance = 12i64;
    assert!(
        (mean as i64 - truth).abs() <= tolerance,
        "RFB frame mean {mean} disagrees with the QMP screendump mean {truth} \
         by more than {tolerance}; the two read the same framebuffer, so one \
         of them is wrong"
    );

    // The desktop may legitimately be black, but it must not be *structurally*
    // empty: a real frame always has the guest's geometry and a decode that
    // produced something. Uniformity is only suspicious next to a non-black
    // truth, which the comparison above already covers.
    eprintln!(
        "OK: {width}x{height} over RFB from a live guest, {:.1}% non-black, \
         mean {mean} (QMP screendump mean {truth})",
        100.0 * nonblack as f64 / total as f64
    );
}

/// Mean luminance of a QMP `screendump`, used as ground truth for the RFB path.
///
/// Goes through the same QMP client `CaptureSource` already holds. Opening a
/// second one is not an option: QEMU's QMP endpoint serves one client at a
/// time, and a second connect is refused -- which is also why the two tests in
/// this file take turns via `QMP_SLOT`.
async fn qmp_screendump_mean(client: &std::sync::Arc<continuum_transport::qmp::QmpClient>) -> i64 {
    let path = std::env::temp_dir().join(format!(
        "continuum-rfb-truth-{}-{}.png",
        std::process::id(),
        qmp().port()
    ));
    let request = serde_json::json!({ "filename": path.to_string_lossy() });
    client
        .execute("screendump", request.into())
        .await
        .expect("QMP screendump for ground truth");

    let bytes = tokio::fs::read(&path)
        .await
        .unwrap_or_else(|e| panic!("reading screendump at {}: {e}", path.display()));
    let _ = tokio::fs::remove_file(&path).await;

    let image = image::load_from_memory(&bytes)
        .unwrap_or_else(|e| panic!("screendump is not a decodable image: {e}"))
        .to_luma8();
    let pixels = image.as_raw();
    let sum: u64 = pixels.iter().map(|&p| p as u64).sum();
    (sum / pixels.len().max(1) as u64) as i64
}

/// The fallback is a real configuration, so it gets tested as one.
///
/// A VM launched without `-vnc` must still stream. If the RFB connect fails
/// for any reason, `CaptureSource` must produce a working QMP source rather
/// than erroring — otherwise adding `-vnc` to a launcher becomes a way to
/// break streaming rather than to speed it up.
///
/// # Why this previously proved nothing
///
/// The earlier version of this test called `CaptureSource::connect`, which
/// hardcoded the real RFB port. So it did not test the fallback at all: with a
/// guest running it silently took the **VNC** path and asserted that input
/// worked, which is true of both paths. It passed either way, and a green suite
/// implied the fallback was covered when it had never been executed.
///
/// `connect_at` now takes the RFB endpoint as a parameter, so pointing it at a
/// closed port actually forces the branch under test.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_without_a_vnc_display_still_streams() {
    let _slot = QMP_SLOT.lock().await;
    // A display nothing is listening on, so the RFB connect must fail. QMP is
    // real, making this exactly the configuration under test: a live guest
    // with no VNC endpoint.
    let dead_display = std::net::SocketAddr::from((
        [127, 0, 0, 1],
        continuum_transport::vnc_port(VNC_DISPLAY + 90),
    ));

    let (source, reason) =
        match CaptureSource::connect_at("omarchy", qmp(), dead_display, 85, 1280, 800).await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("SKIPPED: no guest on QMP 4444: {e}");
                return;
            }
        };

    // The branch must actually have been taken. Without this the test would
    // pass while exercising VNC, which is the mistake it used to make.
    assert_eq!(
        source.name(),
        "qmp",
        "a closed RFB endpoint must select the QMP fallback, and the reason \
         must be reported rather than swallowed"
    );
    let reason = reason.expect("fallback must explain itself so a log can say why");
    assert!(
        reason.contains(&dead_display.port().to_string()),
        "fallback reason {reason:?} should name the RFB endpoint it tried"
    );

    // A source that reports itself as QMP but returns no pixels is not a
    // fallback, it is an outage. This is what actually makes the path real.
    let frame = source
        .capture_jpeg()
        .await
        .expect("the QMP fallback must deliver a frame");
    assert_eq!((frame.width, frame.height), (1280, 800));
    assert!(
        frame.jpeg.len() > 1000,
        "fallback frame is implausibly small: {} bytes",
        frame.jpeg.len()
    );

    // Input must be available over QMP regardless of which video path was
    // chosen -- that is the invariant `CaptureSource` exists to hold. Reaching
    // this line at all *is* the proof the handshake happened inside `connect`.
    assert_eq!(
        source.client().addr().port(),
        4444,
        "input must be available over QMP regardless of which video path was chosen"
    );
    eprintln!(
        "OK: forced fallback to {} delivered a {}x{} JPEG ({} bytes); reason: {reason}",
        source.name(),
        frame.width,
        frame.height,
        frame.jpeg.len()
    );
}
