// Isolate where a black frame appears: capture side or RFB server side.
//
//   cargo run -p continuum-transport --example vnc_path_probe
//
// The end-to-end test reported that a client received an all-black frame while
// a QMP screendump of the same guest was not black. Three places could own
// that fault -- the capture backend, the server's encode-and-send, or the
// client's decode -- and "the test failed" does not distinguish them. So this
// prints the luminance at each hop for the same guest.
use std::sync::Arc;
use std::time::Duration;

use continuum_transport::capture_source::CaptureSource;
use continuum_transport::input_qmp::QmpInputInjector;
use continuum_transport::rfb::{RfbServer, RfbServerConfig};

fn qmp() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 4444))
}

fn mean_luma(jpeg: &[u8]) -> f64 {
    let img = image::load_from_memory(jpeg)
        .expect("decode jpeg")
        .to_luma8();
    let px = img.as_raw();
    px.iter().map(|&p| p as f64).sum::<f64>() / px.len().max(1) as f64
}

#[tokio::main]
async fn main() {
    let (source, fell_back) = CaptureSource::connect("omarchy", qmp(), 85, 1280, 800)
        .await
        .expect("connect");
    println!("video source: {} ({:?})", source.name(), fell_back.is_some());

    // Hop 1 is deliberately *not* measured before the server starts. A capture
    // consumes the single frame the VNC slot holds, and on a still desktop
    // there is no second one -- so probing the capture side first starves the
    // server of the only frame it will ever see and the client below is then
    // judged on a black screen that says nothing about the server.
    //
    // Real usage starts the server and then connects viewers, so that is the
    // order used here.
    let client = source.client();
    let injector = Arc::new(QmpInputInjector::new("omarchy", client, None).with_guest_size(1280, 800));
    let config = RfbServerConfig::secure("127.0.0.1:0".parse().unwrap(), "hunter2");
    let server = RfbServer::new(config, Arc::new(source), injector);
    let listener = server.bind().await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let serving = server.clone();
    tokio::spawn(async move { serving.serve_with(listener).await });

    let addr2 = addr;
    let client_mean = tokio::task::spawn_blocking(move || {
        let mut options = vnc_client::SessionOptions::default();
        options.handshake.security = vec![rfb_proto::types::SecurityType::VncAuth];
        options.handshake.password = Some(b"hunter2".to_vec());
        let mut conn = vnc_client::net::Connection::connect(addr2, options, Duration::from_secs(10))
            .expect("client connect");
        // One painted update is enough. Asking for a second would block forever on a
        // still desktop, because RFB correctly sends nothing when nothing
        // changes.
        let mut got_pixels = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        while !got_pixels && std::time::Instant::now() < deadline {
            if conn
                .wait_for(Duration::from_secs(5), |e| {
                    matches!(e, vnc_client::Event::Updated { .. })
                })
                .is_ok()
            {
                // An update can complete without carrying pixels (a size
                // change or a cursor shape), which is what produced the black
                // first frame this probe was written to find.
                got_pixels = conn.session.has_pixels();
            }
        }
        if !got_pixels {
            return (0, 0, 0);
        }
        let fb = conn.session.framebuffer();
        let data = fb.pixels();
        let sum: u64 = data.iter().map(|&p| ((p & 0xFF) as u64)).sum();
        (fb.width(), fb.height(), sum / data.len().max(1) as u64)
    })
    .await
    .expect("client task");

    println!(
        "rfb client: {}x{} mean blue channel {}",
        client_mean.0, client_mean.1, client_mean.2
    );
}