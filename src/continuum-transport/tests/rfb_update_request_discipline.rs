//! The RFB reader must keep exactly one update request outstanding.
//!
//! # The bug this locks down
//!
//! RFB is request/response: a server sends a framebuffer update *only* in
//! answer to a `FramebufferUpdateRequest`. `Connection::pump_once` does not
//! send one itself, so if the reader stops asking, the guest sends the first
//! frame and then nothing, for ever.
//!
//! A previous version of `capture_vnc.rs` had a comment asserting that
//! `pump_once` "re-requests incrementally thereafter", which is not true -- it
//! does not. The code was then "fixed" by re-requesting on every read,
//! including after an idle timeout.
//!
//! That fix is itself suspect, and this test exists to decide it rather than
//! to assume. `SessionOptions::continuous_updates` already defaults to `true`
//! and issues the next incremental request after each complete update, which
//! means a request is *already* outstanding at all times after the first
//! frame. Re-requesting on top of that leaves two pending, so the server
//! answers both, and the reader decodes and JPEG-encodes the same pixels
//! twice -- retransmitting an unchanged frame, which is the exact cost this
//! backend exists to avoid.
//!
//! # The outcome
//!
//! Measured, not assumed: with the re-request in place an idle desktop
//! accumulated 2 outstanding requests, and without it, 1. The re-request was
//! removed. The original stall that prompted it was never a re-request
//! problem — it was a static screen (rejected pointer input on a wedged
//! guest), which is the kind of mistake that looks like a protocol bug and
//! costs a whole debugging session.
//!
//! # What is asserted, and why a fake server is honest here
//!
//! The fake sends the handshake and then deliberately sends **no updates at
//! all**, which is what a real guest does to a still desktop. A correct client
//! therefore has exactly one request outstanding and sends no more. Counting
//! requests on the wire measures the contract directly.
//!
//! This is not the self-agreement problem that earlier tests in this project
//! fell into. Those compared an encoder with a decoder that shared its
//! assumptions, or checked pixels produced by code under test against code
//! under test. Here the quantity is not pixels at all: it is how many times
//! the client writes a 10-byte request header, which no amount of shared
//! misunderstanding can hide. Pixel correctness is covered against the real
//! guest in `rfb_vnc_end_to_end.rs`.
//!
//! Two details make this test able to fail at all, and both were found by it
//! reporting something other than the expected result:
//!
//! * `VncCaptureBackend::connect` blocks the calling thread, so under the
//!   single-threaded runtime `#[tokio::test]` uses by default it starves the
//!   very task that must answer its handshake. It needs `flavor = "multi_thread"`.
//! * `ServerInit`'s pixel format is 16 bytes, with three *16-bit* colour
//!   maxima. Two bytes short, a client reading a fixed-size struct waits
//!   forever for a name length, and the whole thing presents as a handshake
//!   timeout with no hint of the cause.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use continuum_transport::capture_vnc::VncCaptureBackend;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const WIDTH: u16 = 64;
const HEIGHT: u16 = 48;

/// `FramebufferUpdateRequest` is client message type 3 and is always 10 bytes.
const FB_UPDATE_REQUEST_TYPE: u8 = 3;

/// Counts the update requests the client sends, then holds the connection open.
async fn spawn_counting_rfb_server(requests: Arc<AtomicUsize>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();

        // Version.
        socket.write_all(b"RFB 003.008\n").await.unwrap();
        let mut version = [0u8; 12];
        socket.read_exact(&mut version).await.unwrap();

        // Security: offer None and VNC Auth, and honour whichever is chosen.
        socket.write_all(&[2u8, 1, 2]).await.unwrap();
        let mut chosen = [0u8; 1];
        socket.read_exact(&mut chosen).await.unwrap();
        if chosen[0] == 2 {
            let challenge = [7u8; 16];
            socket.write_all(&challenge).await.unwrap();
            let mut response = [0u8; 16];
            socket.read_exact(&mut response).await.unwrap();
        }
        // SecurityResult: OK.
        socket.write_all(&0u32.to_be_bytes()).await.unwrap();

        // ClientInit: shared-flag byte.
        let mut init = [0u8; 1];
        socket.read_exact(&mut init).await.unwrap();

        // ServerInit: geometry, a 32bpp true-colour format, and an empty name.
        //
        // The pixel format is exactly 16 bytes: bpp, depth, big-endian-flag,
        // true-colour-flag, then *three* 16-bit maxima, three 8-bit shifts and
        // three padding bytes. Writing the maxima as three bytes leaves
        // ServerInit two bytes short, and a client reading a fixed-size struct
        // then blocks forever waiting for a name length that never arrives --
        // which presents as a handshake timeout and says nothing about why.
        let mut server_init = Vec::new();
        server_init.extend_from_slice(&WIDTH.to_be_bytes());
        server_init.extend_from_slice(&HEIGHT.to_be_bytes());
        server_init.extend_from_slice(&[32, 24, 0, 1]); // bpp, depth, big-endian, true-colour
        server_init.extend_from_slice(&255u16.to_be_bytes()); // red max
        server_init.extend_from_slice(&255u16.to_be_bytes()); // green max
        server_init.extend_from_slice(&255u16.to_be_bytes()); // blue max
        server_init.extend_from_slice(&[16, 8, 0]); // red, green, blue shift
        server_init.extend_from_slice(&[0, 0, 0]); // padding
        server_init.extend_from_slice(&0u32.to_be_bytes()); // name length
        socket.write_all(&server_init).await.unwrap();

        // Then send nothing at all: a still desktop. Count what the client asks
        // for, and stay connected so the session is not merely torn down.
        let mut header = [0u8; 1];
        loop {
            match socket.read_exact(&mut header).await {
                Ok(_) => {}
                Err(_) => return,
            }
            if header[0] == FB_UPDATE_REQUEST_TYPE {
                let mut rest = [0u8; 9];
                if socket.read_exact(&mut rest).await.is_err() {
                    return;
                }
                requests.fetch_add(1, Ordering::Relaxed);
            }
            // Any other client message type is ignored; its body length is not
            // parsed, which is safe because this client sends only requests.
        }
    });

    addr
}

/// A still desktop must leave exactly one request outstanding.
///
/// If the reader re-requests on idle, this count climbs into the dozens over a
/// couple of seconds -- each one a duplicate the server will eventually answer,
/// so each one is a redundant encode and a redundant frame on the wire.
// `VncCaptureBackend::connect` blocks the calling thread while it handshakes, so
// on the single-threaded runtime that `#[tokio::test]` uses by default it
// starves the fake server task that has to answer it: the connect times out
// with the server never having run a single step. Multi-threaded so the
// handshake and the server can make progress at the same time.
#[tokio::test(flavor = "multi_thread")]
async fn a_still_desktop_leaves_exactly_one_update_request_outstanding() {
    let requests = Arc::new(AtomicUsize::new(0));
    let addr = spawn_counting_rfb_server(Arc::clone(&requests)).await;

    let _backend = VncCaptureBackend::connect("still", addr, 80, WIDTH as u32, HEIGHT as u32)
        .expect("handshake should succeed against the fake server");

    // The initial non-incremental request is sent during the handshake.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after_connect = requests.load(Ordering::Relaxed);

    // Sit idle for longer than the reader's read timeout, so the timeout path
// actually runs. An earlier version of this test idled for 2.5s against a
// 10s timeout, so it never reached the branch under test and passed without
// saying anything about it -- the same shape of vacuous green as the missing
// fallback test.
    let idle = std::time::Duration::from_millis(13_000);
    tokio::time::sleep(idle).await;
    let after_idle = requests.load(Ordering::Relaxed);

    eprintln!(
        "update requests outstanding: {after_connect} after connect, {after_idle} after 13s idle"
    );

    assert_eq!(
        after_connect, 1,
        "the handshake should leave exactly one update request outstanding"
    );
    assert_eq!(
        after_idle, 1,
        "a still desktop must not provoke further requests: the client sent \
         {after_idle} in total. continuous_updates already keeps one request \
         outstanding after the first frame, so re-requesting on idle leaves \
         duplicates that the server will answer with identical pixels -- which \
         is the redundant re-encode this backend exists to avoid"
    );
}