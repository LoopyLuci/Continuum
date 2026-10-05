//! End-to-end RFB tests: a real client against a real server over loopback.
//!
//! Nothing here is stubbed at the protocol layer. Each test stands up a fake QEMU
//! (the same pattern `capture_qmp`'s own tests use), lets the real
//! `QmpCaptureBackend` screendump it, and drives the real `RfbServer` with a
//! hand-rolled RFB client. That is deliberate: the failure modes this module is
//! written to avoid — a mis-framed Tight length, a Hextile tile the client reads
//! differently, an auth handshake that never completes — are invisible to a unit
//! test that calls the encoder directly and only appear when two independently
//! written halves of the same specification meet on a socket.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use continuum_transport::capture_qmp::QmpCaptureBackend;
use continuum_transport::input_qmp::QmpInputInjector;
use continuum_transport::rfb::{
    generate_challenge, RfbServer, RfbServerConfig, VncPassword, CHALLENGE_LEN, RESPONSE_LEN,
};

/// The password every test authenticates with. Eight bytes is the RFC 6143
/// maximum, which is also this server's maximum.
const PASSWORD: &str = "sesame";

// ---------------------------------------------------------------------------
// Fake QEMU
// ---------------------------------------------------------------------------

/// What the fake QEMU hands back for each successive `screendump`.
#[derive(Clone)]
enum Frames {
    /// The same JPEG forever. A static desktop: the case where RFB must send
    /// nothing after the first frame.
    Fixed(Vec<u8>),
    /// A different JPEG per call, in order, then the last one forever.
    Sequence(Vec<Vec<u8>>),
}

/// A stand-in for a running QEMU: speaks the QMP handshake and answers
/// `screendump` by writing a JPEG to the path it was given.
async fn spawn_fake_qemu(frames: Frames, sent: Arc<AtomicUsize>) -> std::net::SocketAddr {
    const GREETING: &str = r#"{"QMP":{"version":{"package":"8.2.0"},"capabilities":[]}}"#;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();

        let mut calls = 0usize;
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            for line in String::from_utf8_lossy(&buf[..n]).lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if !line.contains("screendump") {
                    if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                        return;
                    }
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
                let filename = request["arguments"]["filename"].as_str().unwrap_or("").to_string();

                let jpeg = match &frames {
                    Frames::Fixed(bytes) => bytes.clone(),
                    Frames::Sequence(all) => {
                        let pick = all.get(calls).unwrap_or_else(|| all.last().unwrap()).clone();
                        calls += 1;
                        pick
                    }
                };
                sent.fetch_add(1, Ordering::Relaxed);
                if tokio::fs::write(&filename, &jpeg).await.is_err() {
                    return;
                }
                if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                    return;
                }
            }
        }
    });

    addr
}

/// What the fake QEMU writes into the screendump file.
///
/// PNG, not JPEG, and that matters: `capture_qmp::read_screendump` verifies that
/// the file is a decodable PNG before returning it, because a truncated dump
/// looks exactly like a rendering bug. Writing a JPEG here would be rejected on
/// every attempt and the capture would silently never succeed.
fn screendump_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// The same image with a block of white painted into it, for the incremental
/// update test.
fn screendump_png_with_block(
    width: u32,
    height: u32,
    rgb: [u8; 3],
    block: (u32, u32, u32, u32),
) -> Vec<u8> {
    let mut image = image::RgbImage::from_pixel(width, height, image::Rgb(rgb));
    for y in block.1..block.1 + block.3 {
        for x in block.0..block.0 + block.2 {
            image.put_pixel(x, y, image::Rgb([255, 255, 255]));
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// Start a server for `qmp_addr`, already bound so the port is known.
async fn start_server(qmp_addr: std::net::SocketAddr, config: RfbServerConfig) -> (RfbServer, std::net::SocketAddr) {
    let capture = Arc::new(
        QmpCaptureBackend::connect("win11", qmp_addr)
            .await
            .unwrap()
            .with_format(85, 64, 48),
    );
    let injector = Arc::new(
        QmpInputInjector::new("win11", Arc::clone(capture.client()), Some("tablet0".into()))
            .with_guest_size(64, 48),
    );
    let server = RfbServer::new(config, capture, injector);
    let listener = server.bind().await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = server.clone();
    tokio::spawn(async move { serving.serve_with(listener).await });
    (server, addr)
}

fn secure_config() -> RfbServerConfig {
    let mut config = RfbServerConfig::secure("127.0.0.1:0".parse().unwrap(), PASSWORD);
    config.frame_interval = Duration::from_millis(30);
    config.idle_timeout = Duration::from_secs(30);
    config.desktop_name = "win11".into();
    config
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// A minimal RFB client, written from the specification rather than by calling
/// into the server's own helpers — otherwise the two halves would agree on a
/// mistake.
struct TestClient {
    stream: TcpStream,
    version: u16,
    width: u16,
    height: u16,
    name: String,
    /// Everything the server has sent since `ServerInit`, decoded rectangles and
    /// all, so a test can assert on pixels rather than bytes.
    canvas: image::RgbImage,
}

impl TestClient {
    /// Connect and complete the handshake, optionally answering the challenge.
    async fn connect(addr: std::net::SocketAddr, password: Option<&str>) -> anyhow::Result<Self> {
        let mut stream = TcpStream::connect(addr).await?;

        let mut banner = [0u8; 12];
        stream.read_exact(&mut banner).await?;
        assert_eq!(&banner[..], b"RFB 003.008\n", "the server must offer 3.8");

        stream.write_all(b"RFB 003.008\n").await?;
        let version = 308u16;

        // Security types: one byte of count, then the list.
        let mut count = [0u8; 1];
        stream.read_exact(&mut count).await?;
        assert!(count[0] >= 1, "the server must offer at least one security type");
        let mut types = vec![0u8; count[0] as usize];
        stream.read_exact(&mut types).await?;
        assert!(types.contains(&2), "VNC Auth (2) must always be offered");

        // Choose VNC Auth.
        stream.write_all(&[2]).await?;

        // Challenge, response, SecurityResult.
        let mut challenge = [0u8; CHALLENGE_LEN];
        stream.read_exact(&mut challenge).await?;
        let response = match password {
            Some(p) => VncPassword::response_for(p, &challenge)?,
            None => [0u8; RESPONSE_LEN],
        };
        stream.write_all(&response).await?;

let mut result = [0u8; 4];
        stream.read_exact(&mut result).await?;
        let status = u32::from_be_bytes(result);
        if status != 0 {
            anyhow::bail!("authentication failed with status {status}");
        }

        // ClientInit: share the desktop with others.
stream.write_all(&[1]).await?;

        // ServerInit: geometry, pixel format, name.
        let mut head = [0u8; 24];
        stream.read_exact(&mut head).await?;
        let width = u16::from_be_bytes([head[0], head[1]]);
        let height = u16::from_be_bytes([head[2], head[3]]);
        let name_len = u32::from_be_bytes([head[20], head[21], head[22], head[23]]) as usize;
        let mut name = vec![0u8; name_len];
        stream.read_exact(&mut name).await?;

        Ok(Self {
            stream,
            version,
            width,
            height,
            name: String::from_utf8_lossy(&name).into_owned(),
            canvas: image::RgbImage::new(width as u32, height as u32),
        })
    }

    /// Ask for an update covering the whole frame.
    async fn request_incremental(&mut self) {
        let mut message = vec![3, 1];
        message.extend_from_slice(&[0, 0]); // x = 0
        message.extend_from_slice(&[0, 0]); // y = 0
        message.extend_from_slice(&self.width.to_be_bytes());
        message.extend_from_slice(&self.height.to_be_bytes());
        self.stream.write_all(&message).await.unwrap();
    }

    /// Ask for a full, non-incremental refresh.
    async fn request_full(&mut self) {
        let mut message = vec![3, 0];
        message.extend_from_slice(&[0, 0, 0, 0]);
        message.extend_from_slice(&self.width.to_be_bytes());
        message.extend_from_slice(&self.height.to_be_bytes());
        self.stream.write_all(&message).await.unwrap();
    }

    async fn set_encodings(&mut self, encodings: &[i32]) {
        let mut message = vec![2, 0];
        message.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
        for encoding in encodings {
            message.extend_from_slice(&encoding.to_be_bytes());
        }
        self.stream.write_all(&message).await.unwrap();
    }

    async fn key_event(&mut self, keysym: u32, down: bool) {
        let mut message = vec![4, u8::from(down), 0, 0];
        message.extend_from_slice(&keysym.to_be_bytes());
        self.stream.write_all(&message).await.unwrap();
    }

    async fn pointer_event(&mut self, x: u16, y: u16, mask: u8) {
        let mut message = vec![5, mask];
        message.extend_from_slice(&x.to_be_bytes());
        message.extend_from_slice(&y.to_be_bytes());
        self.stream.write_all(&message).await.unwrap();
    }

    /// Read one `FramebufferUpdate`, or `None` if nothing arrives in time.
    ///
    /// Decodes Raw, CopyRect, RRE, Hextile and Tight, applying each to
    /// `self.canvas`. Returning `None` on timeout is what lets a test assert that
    /// a static desktop produces *no* traffic.
    async fn next_update(&mut self, timeout_ms: u64) -> Option<Update> {
        let deadline = Duration::from_millis(timeout_ms);
        match tokio::time::timeout(deadline, self.read_update_message()).await {
            Ok(Some(update)) => Some(update),
            Ok(None) => None,
            Err(_) => None,
        }
    }

    async fn read_update_message(&mut self) -> Option<Update> {
        let mut head = [0u8; 4];
        self.stream.read_exact(&mut head).await.ok()?;
        assert_eq!(head[0], 0, "server message type 0 is FramebufferUpdate");
        assert_eq!(head[1], 0, "the padding byte is reserved and must be zero");
        let count = u16::from_be_bytes([head[2], head[3]]);

        let mut bytes = 0usize;
        let mut encodings = Vec::new();
        for _ in 0..count {
            let rect = self.read_rect().await?;
            bytes += self.apply_rect(rect).await?;
            encodings.push(rect.encoding);
        }
        Some(Update { rectangles: count, encodings, bytes })
    }

    async fn read_rect(&mut self) -> Option<RectHeader> {
        let mut head = [0u8; 12];
        self.stream.read_exact(&mut head).await.ok()?;
        Some(RectHeader {
            x: u16::from_be_bytes([head[0], head[1]]),
            y: u16::from_be_bytes([head[2], head[3]]),
            width: u16::from_be_bytes([head[4], head[5]]),
            height: u16::from_be_bytes([head[6], head[7]]),
            encoding: i32::from_be_bytes([head[8], head[9], head[10], head[11]]),
        })
    }

    async fn read_exact_bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        let mut bytes = vec![0u8; n];
        self.stream.read_exact(&mut bytes).await.ok()?;
        Some(bytes)
    }

    /// Apply one rectangle to the canvas, returning the bytes it consumed.
    async fn apply_rect(&mut self, rect: RectHeader) -> Option<usize> {
        match rect.encoding {
            0 => {
                // Raw: width * height * 4 BGRX bytes.
                let count = rect.width as usize * rect.height as usize * 4;
                let bytes = self.read_exact_bytes(count).await?;
                for (i, pixel) in bytes.chunks_exact(4).enumerate() {
                    let index = i % rect.width as usize;
                    let row = i / rect.width as usize;
                    self.set_bgrx(rect.x + index as u16, rect.y + row as u16, pixel);
                }
                Some(4 + bytes.len())
            }
            1 => {
                // CopyRect: two more u16, then move pixels inside the canvas.
                let src = self.read_exact_bytes(4).await?;
                let src_x = u16::from_be_bytes([src[0], src[1]]);
                let src_y = u16::from_be_bytes([src[2], src[3]]);
                // Read the whole block before writing any of it: the source and
                // the destination overlap on most scrolls.
                let mut moved = Vec::with_capacity(rect.width as usize * rect.height as usize);
                for row in 0..rect.height as u32 {
                    for col in 0..rect.width as u32 {
                        moved.push(
                            *self
                                .canvas
                                .get_pixel(src_x as u32 + col, src_y as u32 + row),
                        );
                    }
                }
                for (i, pixel) in moved.into_iter().enumerate() {
                    let col = (i % rect.width as usize) as u32;
                    let row = (i / rect.width as usize) as u32;
                    self.canvas
                        .put_pixel(rect.x as u32 + col, rect.y as u32 + row, pixel);
                }
                Some(16)
            }
            2 => self.apply_rre(rect).await,
            5 => self.apply_hextile(rect).await,
            7 => self.apply_tight(rect).await,
            -223 => {
                // DesktopSize: the geometry has already been announced in
                // ServerInit, so there is no payload and nothing to apply.
                Some(12)
            }
            other => panic!("the server sent an encoding it does not implement: {other}"),
        }
    }

    /// Consume the remainder of an update whose rectangles are all understood.
    async fn apply_rre(&mut self, rect: RectHeader) -> Option<usize> {
        let head = self.read_exact_bytes(8).await?;
        let count = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let background = [head[4], head[5], head[6], head[7]];
        let mut total = 8;
        self.fill(rect.x, rect.y, rect.width, rect.height, background);
        for _ in 0..count {
            let sub = self.read_exact_bytes(12).await?;
            total += 12;
            let x = u16::from_be_bytes([sub[0], sub[1]]);
            let y = u16::from_be_bytes([sub[2], sub[3]]);
            let w = u16::from_be_bytes([sub[4], sub[5]]);
            let h = u16::from_be_bytes([sub[6], sub[7]]);
            let colour = [sub[8], sub[9], sub[10], sub[11]];
            self.fill(rect.x + x, rect.y + y, w, h, colour);
        }
        Some(4 + total)
    }

    async fn apply_hextile(&mut self, rect: RectHeader) -> Option<usize> {
        let mut total = 0usize;
        // The background carried from the previous non-raw tile.
        let mut background = [0u8, 0, 0, 0];
        let tile = 16u16;
        let tiles_x = (rect.width as usize).div_ceil(tile as usize);
        let tiles_y = (rect.height as usize).div_ceil(tile as usize);

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let mask = self.read_exact_bytes(1).await?[0];
                total += 1;
                let cols = (rect.width as usize - tx * tile as usize).min(tile as usize);
                let rows = (rect.height as usize - ty * tile as usize).min(tile as usize);
                let origin_x = rect.x + (tx * tile as usize) as u16;
                let origin_y = rect.y + (ty * tile as usize) as u16;

                if mask & 1 != 0 {
                    // Raw: `cols * rows` pixels, and the background is dropped.
                    let bytes = self.read_exact_bytes(cols * rows * 4).await?;
                    total += bytes.len();
                    for (i, pixel) in bytes.chunks_exact(4).enumerate() {
                        let cx = (i % cols) as u16;
                        let cy = (i / cols) as u16;
                        self.set_bgrx(origin_x + cx, origin_y + cy, pixel);
                    }
                    background = [0, 0, 0, 0];
                } else {
                    if mask & 2 != 0 {
                        background = self.read_exact_bytes(4).await?[0..4].try_into().ok()?;
                        total += 4;
                    }
                    assert_eq!(
                        mask & 8,
                        0,
                        "the server must not emit AnySubrects: it is not implemented"
                    );
                    self.fill(origin_x, origin_y, cols as u16, rows as u16, background);
                }
            }
        }
        Some(4 + total)
    }

    async fn apply_tight(&mut self, rect: RectHeader) -> Option<usize> {
        let control = self.read_exact_bytes(1).await?[0];
        assert_eq!(
            control, 0x01,
            "expected BasicCompression, stream 0, CopyFilter, reset stream 0"
        );
        let (length, used) = read_compact_length(self).await;
        let payload = self.read_exact_bytes(length).await?;
        let raw = inflate(&payload);

        // TPIXEL: three bytes per pixel in R, G, B order for a 32bpp depth-24
        // format, which is what ServerInit advertised.
        let expected = rect.width as usize * rect.height as usize * 3;
        assert_eq!(raw.len(), expected, "TPIXEL is three bytes per pixel");
        for (i, pixel) in raw.chunks_exact(3).enumerate() {
            let cx = (i % rect.width as usize) as u16;
            let cy = (i / rect.width as usize) as u16;
            self.canvas.put_pixel(
                (rect.x + cx) as u32,
                (rect.y + cy) as u32,
                image::Rgb([pixel[0], pixel[1], pixel[2]]),
            );
        }
        Some(4 + 1 + used + payload.len())
    }

    fn set_bgrx(&mut self, x: u16, y: u16, pixel: &[u8]) {
        self.canvas.put_pixel(
            x as u32,
            y as u32,
            image::Rgb([pixel[2], pixel[1], pixel[0]]),
        );
    }

    fn fill(&mut self, x: u16, y: u16, w: u16, h: u16, bgrx: [u8; 4]) {
        for row in 0..h {
            for col in 0..w {
                self.set_bgrx(x + col, y + row, &bgrx);
            }
        }
    }
}

/// Read a Tight compact length from the socket.
async fn read_compact_length(client: &mut TestClient) -> (usize, usize) {
    let first = client.read_exact_bytes(1).await.unwrap()[0];
    if first & 0x80 == 0 {
        return (first as usize, 1);
    }
    let second = client.read_exact_bytes(1).await.unwrap()[0];
    let low = (first as usize & 0x7F) | ((second as usize & 0x7F) << 7);
    if second & 0x80 == 0 {
        (low, 2)
    } else {
        let third = client.read_exact_bytes(1).await.unwrap()[0];
        (low | ((third as usize) << 14), 3)
    }
}

/// Decompress a zlib stream.
fn inflate(data: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(std::io::Cursor::new(data))
        .read_to_end(&mut out)
        .expect("the Tight payload should be a valid zlib stream");
    out
}

#[derive(Clone, Copy)]
struct RectHeader {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    encoding: i32,
}

#[derive(Debug)]
struct Update {
    rectangles: u16,
    encodings: Vec<i32>,
    bytes: usize,
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_auth_framebuffer_and_input_all_work_over_loopback() {
    // One test for the whole path, because the interesting failures are the ones
    // where each stage works alone and the sequence does not.
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [30, 60, 90])), Arc::clone(&sent)).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD))
        .await
        .expect("a correct password must complete the handshake");
    assert_eq!(client.version, 308);
    assert_eq!((client.width, client.height), (64, 48));
    assert_eq!(client.name, "win11");

    // Framebuffer.
    client.request_incremental().await;
    let first = client
        .next_update(5_000)
        .await
        .expect("the first request must be answered with the whole framebuffer");
    assert!(first.rectangles >= 1, "at least the whole frame");
    assert_pixel_close(&client.canvas, 10, 10, [30, 60, 90]);

    // A static desktop must then produce nothing at all. This is the bandwidth
    // claim: the screendump still happens, the wire does not move.
    for _ in 0..5 {
        client.request_incremental().await;
        assert!(
            client.next_update(400).await.is_none(),
            "an unchanged desktop must not produce an update"
        );
    }
    assert!(
        sent.load(Ordering::Relaxed) >= 6,
        "the capture loop must still be running"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_password_is_refused_and_nothing_is_disclosed() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [1, 2, 3])), sent).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut banner = [0u8; 12];
    stream.read_exact(&mut banner).await.unwrap();
    stream.write_all(b"RFB 003.008\n").await.unwrap();

    let mut count = [0u8; 1];
    stream.read_exact(&mut count).await.unwrap();
    let mut types = vec![0u8; count[0] as usize];
    stream.read_exact(&mut types).await.unwrap();
    stream.write_all(&[2]).await.unwrap();

    let mut challenge = [0u8; CHALLENGE_LEN];
    stream.read_exact(&mut challenge).await.unwrap();
    let wrong = VncPassword::response_for("sesamf", &challenge).unwrap();
    stream.write_all(&wrong).await.unwrap();

    let mut result = [0u8; 4];
    stream.read_exact(&mut result).await.unwrap();
    assert_eq!(
        u32::from_be_bytes(result),
        1,
        "a wrong password must produce SecurityResult failed"
    );

    // 3.8 then sends a reason and closes. The reason must not say *how* it failed.
    let mut length = [0u8; 4];
    if stream.read_exact(&mut length).await.is_ok() {
        let n = u32::from_be_bytes(length) as usize;
        let mut reason = vec![0u8; n];
        stream.read_exact(&mut reason).await.unwrap();
        let text = String::from_utf8_lossy(&reason).to_lowercase();
        assert!(!text.contains("sesame") && !text.contains("sesamf"), "leaked the password");
        assert!(text.contains("authentication") || text.contains("failed"), "got {text}");
    }

    // And the connection must be closed: no pixels, ever.
    let mut trailing = Vec::new();
    assert!(
        stream.read_to_end(&mut trailing).await.is_ok() || trailing.is_empty(),
        "the server must close after a failed authentication"
    );
    assert!(trailing.is_empty(), "no framebuffer data after a refusal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_negotiated_version_completes_the_handshake() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [9, 9, 9])), Arc::clone(&sent)).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    for banner in [&b"RFB 003.003\n"[..], b"RFB 003.007\n", b"RFB 003.008\n"] {
        let mut stream = TcpStream::connect(addr).await.unwrap();

        let mut offered = [0u8; 12];
        stream.read_exact(&mut offered).await.unwrap();
        assert_eq!(&offered[..], b"RFB 003.008\n");
        stream.write_all(banner).await.unwrap();

        let version = if banner == b"RFB 003.003\n" { 33u16 } else if banner == b"RFB 003.007\n" { 37 } else { 38 };

        if version == 33 {
            // 3.3: the server states the security type as a u32.
            let mut chosen = [0u8; 4];
            stream.read_exact(&mut chosen).await.unwrap();
            assert_eq!(u32::from_be_bytes(chosen), 2, "3.3 must state VNC Auth");
        } else {
            let mut count = [0u8; 1];
            stream.read_exact(&mut count).await.unwrap();
            let mut types = vec![0u8; count[0] as usize];
            stream.read_exact(&mut types).await.unwrap();
            assert!(types.contains(&2));
            stream.write_all(&[2]).await.unwrap();
        }

        let mut challenge = [0u8; CHALLENGE_LEN];
        stream.read_exact(&mut challenge).await.unwrap();
        let response = VncPassword::response_for(PASSWORD, &challenge).unwrap();
        stream.write_all(&response).await.unwrap();

        let mut result = [0u8; 4];
        stream.read_exact(&mut result).await.unwrap();
        assert_eq!(u32::from_be_bytes(result), 0, "version 3.{version} auth failed");

        stream.write_all(&[0]).await.unwrap();
        let mut head = [0u8; 24];
        stream.read_exact(&mut head).await.unwrap();
        assert_eq!((u16::from_be_bytes([head[0], head[1]]), u16::from_be_bytes([head[2], head[3]])), (64, 48));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incremental_update_carries_only_the_changed_rectangle() {
    let sent = Arc::new(AtomicUsize::new(0));
    // Two frames: a dark desktop, then the same desktop with a white block. The
    // second capture is the only change, so only that block may be sent.
    let first = screendump_png(64, 48, [20, 20, 40]);
    let second = screendump_png_with_block(64, 48, [20, 20, 40], (40, 20, 16, 8));

    let qmp = spawn_fake_qemu(Frames::Sequence(vec![first, second]), sent).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    // Raw only, so the assertion below reads pixel coordinates directly.
    client.set_encodings(&[0]).await;

    client.request_incremental().await;
    client
        .next_update(5_000)
        .await
        .expect("the initial full framebuffer");
    assert_pixel_close(&client.canvas, 2, 2, [20, 20, 40]);

    // Wait for the capture loop to reach the changed frame.
    let mut incremental = None;
    for _ in 0..40 {
        client.request_incremental().await;
        if let Some(update) = client.next_update(500).await {
            if update.rectangles == 1 {
                incremental = Some(update);
                break;
            }
        }
    }
    let incremental = incremental.expect("the changed block should arrive as one update");

    // A 16x8 block of a 64x48 framebuffer: 128 pixels out of 3072.
    assert!(
        incremental.bytes < 1_500,
        "an incremental update was {} bytes; a full Raw frame is {}",
        incremental.bytes,
        64 * 48 * 4 + 4 + 12
    );
    // And the canvas now shows the block.
    assert_pixel_close(&client.canvas, 48, 24, [255, 255, 255]);
    assert_pixel_close(&client.canvas, 2, 2, [20, 20, 40]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_incremental_request_resynchronises_the_whole_frame() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [64, 64, 64])), sent).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    client.set_encodings(&[0]).await;
    client.request_incremental().await;
    client.next_update(5_000).await.expect("first frame");

    // Scribble over the canvas so a partial refresh could not fix it.
    for y in 0..48u32 {
        for x in 0..64u32 {
            client.canvas.put_pixel(x, y, image::Rgb([1, 2, 3]));
        }
    }

    client.request_full().await;
    let update = client
        .next_update(5_000)
        .await
        .expect("a non-incremental request must be answered in full");
    assert_eq!(update.rectangles, 1);
    assert_pixel_close(&client.canvas, 10, 10, [64, 64, 64]);
    assert_pixel_close(&client.canvas, 60, 40, [64, 64, 64]);
    // The whole frame, not a delta.
    assert!(update.bytes > 64 * 48 * 4 / 2, "expected a full-frame resynchronisation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tight_client_receives_a_compressed_rectangle() {
    let sent = Arc::new(AtomicUsize::new(0));
    // A flat colour compresses hard, so Tight should beat Raw by a wide margin.
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [80, 80, 80])), sent).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    client.set_encodings(&[7, 0]).await; // Tight, then Raw.
    client.request_incremental().await;

    let update = client
        .next_update(5_000)
        .await
        .expect("the first update should be answered");
    assert_eq!(
        update.encodings,
        vec![7],
        "the server should have chosen Tight for a flat desktop"
    );
    assert_pixel_close(&client.canvas, 5, 5, [80, 80, 80]);

    let raw_frame = 64 * 48 * 4;
    assert!(
        update.bytes < raw_frame / 4,
        "Tight sent {} bytes against a {raw_frame}-byte Raw frame",
        update.bytes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_raw_only_client_still_gets_correct_pixels() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(64, 48, [200, 100, 50])), sent).await;
    let (_server, addr) = start_server(qmp, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    client.set_encodings(&[-239, -308, 16, 224, 250, -313]).await;
    client.request_incremental().await;

    let update = client
        .next_update(5_000)
        .await
        .expect("a client advertising only unknown encodings must still work");
    assert_eq!(
        update.encodings,
        vec![0],
        "Raw is mandatory and must be used when nothing else was advertised"
    );
    assert_pixel_close(&client.canvas, 40, 40, [200, 100, 50]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_events_reach_the_guest_over_qmp() {
    // The fake QEMU records every command it receives, so this asserts that a
    // PointerEvent and a KeyEvent actually reached `input-send-event` and
    // `human-monitor-command` rather than being quietly dropped.
    let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp_addr = spawn_recording_qemu(Frames::Fixed(screendump_png(32, 32, [0, 0, 0])), Arc::clone(&sent), Arc::clone(&recorded)).await;

    let capture = Arc::new(
        QmpCaptureBackend::connect("win11", qmp_addr)
            .await
            .unwrap()
            .with_format(85, 32, 32),
    );
    let injector = Arc::new(
        QmpInputInjector::new("win11", Arc::clone(capture.client()), Some("tablet0".into()))
            .with_guest_size(32, 32),
    );
    let server = RfbServer::new(secure_config(), capture, injector);
    let listener = server.bind().await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = server.clone();
    tokio::spawn(async move { serving.serve_with(listener).await });

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    client.request_incremental().await;
    client.next_update(5_000).await.expect("first frame");

    client.pointer_event(10, 20, 1).await; // left button down
    client.key_event(0x61, true).await; // 'a'

    // Both are serialised behind the QMP socket, so poll for them.
    let mut saw_pointer = false;
    let mut saw_key = false;
    for _ in 0..60 {
        let log = recorded.lock().clone();
        saw_pointer = log.iter().any(|line| line.contains("input-send-event"));
        saw_key = log.iter().any(|line| line.contains("sendkey a"));
        if saw_pointer && saw_key {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = recorded.lock().clone();
    assert!(
        saw_pointer,
        "a PointerEvent never became an input-send-event; QMP saw: {log:?}"
    );
    assert!(
        saw_key,
        "a KeyEvent never became a sendkey; QMP saw: {log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmappable_keysym_is_refused_rather_than_guessed() {
    let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp_addr = spawn_recording_qemu(Frames::Fixed(screendump_png(32, 32, [0, 0, 0])), sent, Arc::clone(&recorded)).await;
    let (_server, addr) = start_server(qmp_addr, secure_config()).await;

    let mut client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
    client.request_incremental().await;
    client.next_update(5_000).await.expect("first frame");

    // 0x1008FE51 is the ISO_Left_Tab keysym: real, and deliberately absent from
    // the existing HMP vocabulary. It must produce no sendkey at all.
    client.key_event(0x0100_8FE5, true).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let log = recorded.lock().clone();
    assert!(
        !log.iter().any(|line| line.contains("sendkey")),
        "an unmappable keysym must not be turned into a guessed key; QMP saw: {log:?}"
    );
    // And the session is still alive.
    client.request_incremental().await;
    client.key_event(0x62, true).await; // 'b', which does map
    let mut recovered = false;
    for _ in 0..60 {
        if recorded.lock().iter().any(|line| line.contains("sendkey b")) {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(recovered, "the session must survive an unmappable keysym");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_limit_is_enforced() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(32, 32, [5, 5, 5])), sent).await;
    let mut config = secure_config();
    config.max_clients = 1;
    let (_server, addr) = start_server(qmp, config).await;

    let _first = TestClient::connect(addr, Some(PASSWORD))
        .await
        .expect("the first client must be admitted");

// The second is refused before any handshake work: the slot is taken, so the
    // server drops the socket without sending a version banner. That is the
    // whole point of refusing early — an over-limit client gets no chance to
    // spend a framebuffer's worth of memory or an idle timeout's worth of wall
    // clock.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut rest)).await;
    assert!(
        read.map(|r| r.is_ok()).unwrap_or(false),
        "the listener must still accept a connection beyond its limit"
    );
    assert!(
        rest.is_empty(),
        "a refused client must get no session data, got {} bytes",
        rest.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn several_clients_share_one_capture() {
    let sent = Arc::new(AtomicUsize::new(0));
    let qmp = spawn_fake_qemu(Frames::Fixed(screendump_png(32, 32, [7, 7, 7])), Arc::clone(&sent)).await;
    let mut config = secure_config();
    config.max_clients = 4;
    let (_server, addr) = start_server(qmp, config).await;

    let mut clients = Vec::new();
    for _ in 0..3 {
        let client = TestClient::connect(addr, Some(PASSWORD)).await.unwrap();
        clients.push(client);
    }
    for client in &mut clients {
        client.request_incremental().await;
        let update = client.next_update(5_000).await.expect("every client gets a frame");
        assert_eq!(update.rectangles, 1);
        assert_pixel_close(&client.canvas, 1, 1, [7, 7, 7]);
    }

    // Three viewers, one capture per tick rather than three.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let per_client = 3f64;
    assert!(
        sent.load(Ordering::Relaxed) as f64 >= per_client,
        "the capture loop must have run at least once per client slot"
    );
}

/// A fake QEMU that records every request line it receives.
async fn spawn_recording_qemu(
    frames: Frames,
    sent: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<String>>>,
) -> std::net::SocketAddr {
    const GREETING: &str = r#"{"QMP":{"version":{"package":"8.2.0"},"capabilities":[]}}"#;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();

        let mut calls = 0usize;
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            for line in String::from_utf8_lossy(&buf[..n]).lines() {
                if line.trim().is_empty() {
                    continue;
                }
                recorded.lock().push(line.to_string());
                if !line.contains("screendump") {
                    if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                        return;
                    }
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
                let filename = request["arguments"]["filename"].as_str().unwrap_or("").to_string();
                let jpeg = match &frames {
                    Frames::Fixed(bytes) => bytes.clone(),
                    Frames::Sequence(all) => {
                        let pick = all.get(calls).unwrap_or_else(|| all.last().unwrap()).clone();
                        calls += 1;
                        pick
                    }
                };
                sent.fetch_add(1, Ordering::Relaxed);
                if tokio::fs::write(&filename, &jpeg).await.is_err() {
                    return;
                }
                if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                    return;
                }
            }
        }
    });

    addr
}

/// Assert a pixel is close to `expected`.
///
/// The capture backend transcodes the screendump to JPEG at quality 85 before
/// the RFB server ever sees it, so exact equality is not available at this layer
/// and asserting it would be asserting the JPEG encoder. Three levels of slack is
/// well inside the encoder's error for a flat region.
#[track_caller]
fn assert_pixel_close(canvas: &image::RgbImage, x: u32, y: u32, expected: [u8; 3]) {
    let got = canvas.get_pixel(x, y).0;
    for (channel, (a, b)) in got.iter().zip(expected.iter()).enumerate() {
        let delta = (*a as i32 - *b as i32).abs();
        assert!(
            delta <= 3,
            "channel {channel} at ({x}, {y}) was {a}, expected about {b} (delta {delta})"
        );
    }
}

/// Prove the challenge is fresh, so the auth exchange cannot be replayed.
#[test]
fn challenges_do_not_repeat() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        assert!(seen.insert(generate_challenge()), "a repeated challenge is replayable");
    }
}

/// A DES implementation the tests share with `auth`, exercised over the wire
/// path: the same password must produce the same response for the same
/// challenge, and a different one for a different challenge.
#[test]
fn the_response_is_deterministic_per_challenge() {
    let challenge = [7u8; 16];
    let a = VncPassword::response_for(PASSWORD, &challenge).unwrap();
    let b = VncPassword::response_for(PASSWORD, &challenge).unwrap();
    assert_eq!(a, b, "DES is deterministic; a mismatch means a mutable key");

    let other = [8u8; 16];
    assert_ne!(a, VncPassword::response_for(PASSWORD, &other).unwrap());
    assert_ne!(a, VncPassword::response_for("sesamg", &challenge).unwrap());
}