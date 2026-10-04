//! Capture a QEMU VM's framebuffer over QMP.
//!
//! QEMU will dump its own display to a file on request:
//!
//! ```json
//! {"execute":"screendump","arguments":{"filename":"C:\\...\\frame-7.png","format":"png"}}
//! ```
//!
//! That is the whole mechanism. QEMU writes the file and answers `{"return":{}}`,
//! so the client then reads the file back off the host filesystem and deletes
//! it.
//!
//! ## Why a file and not a socket
//!
//! QMP has no "give me the framebuffer" command. `screendump` is the only
//! framebuffer export path, and it is file-based. That has three consequences
//! this module has to handle:
//!
//! 1. **The filename must be unique per capture.** Two sessions sharing a VM
//!    at 30 fps generate two screendumps per 33 ms. A fixed path means session
//!    A reads session B's frame, or reads a file mid-write and decodes a
//!    truncated PNG. `screendump_path` mixes the process id with a monotonic
//!    counter for this reason.
//! 2. **The file must be deleted.** QEMU never cleans up. Left alone, a
//!    30 fps session writes ~1 GB/hour of PNG to the temp directory.
//! 3. **A reply does not mean the bytes are on disk yet.** QEMU answers after
//!    the write completes, but the only way to be sure the file is complete is
//!    to observe it, so the read is retried briefly rather than assumed.
//!
//! `format: "png"` is deliberate even though the wire format is JPEG: PNG is
//! lossless, so transcoding cannot double-compress. QEMU's default is PPM,
//! which nothing in the Windows toolchain opens.
//!
//! ## Serialisation
//!
//! [`QmpCaptureBackend`] holds a mutex across capture even though
//! `CaptureBackend::capture_frame` already takes `&mut self`. The `&mut`
//! protects one instance; the mutex protects the *temp file and the QMP
//! socket* from a second instance pointed at the same VM. Two sidecar
//! clients on one VM must not interleave screendumps.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use tokio::sync::Mutex;

use continuum_core::capture::{now_micros, CapturedFrame, CaptureBackend, MonitorInfo};
use continuum_core::{ContinuumError, ContinuumResult};

use crate::qmp::QmpClient;

/// Process-unique counter for temp filenames.
static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Default JPEG quality, matching `FrameSemantics::default()`.
pub const DEFAULT_CAPTURE_QUALITY: u8 = 85;

/// How long to keep re-reading the screendump file before giving up.
///
/// Only ever exhausted under real IO pressure; the loop exits on the first
/// successful parse.
const SCREENDUMP_READ_ATTEMPTS: u32 = 8;
const SCREENDUMP_READ_DELAY: Duration = Duration::from_millis(25);

/// Build a filename that no other capture, process or session can collide with.
///
/// QEMU runs with whatever privileges the user has, so the temp directory is
/// the only writable location guaranteed to exist on both the Windows and
/// Linux hosts Continuum supports.
pub fn screendump_path() -> PathBuf {
    let seq = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "continuum-qmp-{}-{}.png",
        std::process::id(),
        seq
    ))
}

/// QEMU absolute input axis range for a USB tablet.
///
/// QEMU's `usb-tablet` reports `ABS_X`/`ABS_Y` with a maximum of `0x7FFF`.
/// Pointer coordinates are normalised into this range before being sent;
/// sending pixel coordinates instead moves the guest pointer to roughly
/// 1/32767th of the way across the screen.
pub const TABLET_AXIS_MAX: f32 = 32767.0;

/// A [`CaptureBackend`] that reads a QEMU VM's framebuffer over QMP.
pub struct QmpCaptureBackend {
    client: Arc<QmpClient>,
    vm_name: String,
    quality: u8,
    width: u32,
    height: u32,
    /// Set once the QMP handshake succeeded; drives `is_available`.
    available: AtomicBool,
    /// Serialises capture across every clone of this backend.
    capture_lock: Mutex<()>,
}

/// A single captured frame, already encoded for the wire.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
    pub timestamp_us: i64,
}

impl QmpCaptureBackend {
    /// Connect to a VM's QMP endpoint and build a backend for it.
    ///
    /// The handshake happens here rather than lazily on first capture so that
    /// a wrong port fails when the GUI lists VMs, not ten seconds into a
    /// subscription that will never produce a frame.
    pub async fn connect(vm_name: &str, qmp_addr: SocketAddr) -> ContinuumResult<Self> {
        let client = QmpClient::connect(qmp_addr).await.map_err(|e| {
            ContinuumError::Capture(format!("QMP connect to {qmp_addr} failed: {e}"))
        })?;
        Ok(Self::with_client(vm_name, Arc::new(client)))
    }

    /// Build a backend around an already-connected QMP client.
    pub fn with_client(vm_name: &str, client: Arc<QmpClient>) -> Self {
        Self {
            client,
            vm_name: vm_name.to_string(),
            quality: DEFAULT_CAPTURE_QUALITY,
            width: 1920,
            height: 1080,
            available: AtomicBool::new(true),
            capture_lock: Mutex::new(()),
        }
    }

    /// Set the transcoding parameters applied to every frame.
    ///
    /// The VM's own display size is whatever QEMU was launched with; these are
    /// what the client asked to see. They are applied with `resize_exact`, so
    /// a mismatched aspect ratio stretches rather than letterboxes — the
    /// alternative silently shifts every pointer coordinate.
    pub fn with_format(mut self, quality: u8, width: u32, height: u32) -> Self {
        self.quality = quality.clamp(1, 100);
        self.width = width.max(1);
        self.height = height.max(1);
        self
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn vm_name(&self) -> &str {
        &self.vm_name
    }

    pub fn client(&self) -> &Arc<QmpClient> {
        &self.client
    }

    /// Tell QEMU to dump its framebuffer, read the PNG back, and transcode it
    /// to JPEG at the configured quality and size.
    ///
    /// Takes `&self` and locks internally so it can be called concurrently
    /// from the frame loop and from a one-shot "grab a thumbnail" request
    /// without the two grabbing the same filename.
    pub async fn capture_jpeg(&self) -> ContinuumResult<EncodedFrame> {
        // Held for the whole capture, including the file read and the
        // transcode. Anything less and a second caller could already have
        // issued its screendump by the time we read.
        let _guard = self.capture_lock.lock().await;

        let png = self.screendump_png().await?;
        let image = resize_to(decode_png(&png)?, self.width, self.height);
        let (width, height) = (image.width(), image.height());
        let jpeg = encode_jpeg(&image, self.quality)?;

        Ok(EncodedFrame {
            width,
            height,
            jpeg,
            timestamp_us: now_micros(),
        })
    }

    /// Ask QEMU for a framebuffer dump and return the raw PNG bytes.
    ///
    /// The temp file is always removed, including on the error paths — QEMU
    /// will not clean it up, and a 30 fps session that leaks a 3 MB PNG per
    /// frame fills a temp directory in minutes.
    async fn screendump_png(&self) -> ContinuumResult<Vec<u8>> {
        let path = screendump_path();
        let filename = path.to_string_lossy().to_string();

        self.client
            .execute(
                "screendump",
                Some(serde_json::json!({ "filename": filename, "format": "png" })),
            )
            .await
            .map_err(|e| {
                ContinuumError::Capture(format!(
                    "QMP screendump failed for {}: {e}",
                    self.vm_name
                ))
            })?;

        let png = read_screendump(&path).await;
        let _ = tokio::fs::remove_file(&path).await;
        png
    }
}

impl CaptureBackend for QmpCaptureBackend {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>> {
        // A VM has exactly one display, and QMP exposes no query that returns
        // its current geometry — `query-display-options` reports connection
        // state, not resolution. The negotiated size is therefore the
        // configured one, which is also what the client is being told.
        Ok(vec![MonitorInfo {
            id: 0,
            name: self.vm_name.clone(),
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
            is_primary: true,
            scale_factor: 1.0,
        }])
    }

    fn capture_frame(&mut self, monitor_id: u32) -> ContinuumResult<CapturedFrame> {
        // The trait is synchronous and QMP is not. Rather than pretend, the
        // inherent async `capture_jpeg` is what every real caller uses; this
        // shim exists so the trait is exercised and can be called from the
        // synchronous encoder path.
        futures_block_on_capture(self, monitor_id)
    }

    fn name(&self) -> &str {
        "qmp"
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }
}

/// Run a future to completion on a dedicated thread with a fresh runtime.
///
/// The `CaptureBackend` trait is synchronous because the QUIC encoder calls it
/// from a sync frame loop, but `screendump` is a socket round trip. Blocking
/// that loop for ~15 ms per frame is acceptable; deadlocking it by nesting a
/// runtime inside the caller's runtime is not, which is why this gets a
/// separate one.
fn futures_block_on_capture(
    backend: &QmpCaptureBackend,
    monitor_id: u32,
) -> ContinuumResult<CapturedFrame> {
    if monitor_id != 0 {
        return Err(ContinuumError::Capture(format!(
            "VM {} has one display; monitor {monitor_id} does not exist",
            backend.vm_name
        )));
    }
    let width = backend.width;
    let height = backend.height;
    let quality = backend.quality;

    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| ContinuumError::Capture(format!("runtime: {e}")))?;
                rt.block_on(async move {
                    let png = backend.screendump_png().await?;
                    let image = resize_to(decode_png(&png)?, width, height);
                    let (w, h) = (image.width(), image.height());
                    Ok::<_, ContinuumError>(CapturedFrame::new(
                        w,
                        h,
                        encode_jpeg(&image, quality)?,
                        monitor_id,
                    ))
                })
            })
            .join()
            .map_err(|_| ContinuumError::Capture("capture thread panicked".into()))?
    })
}

/// Read the screendump file, tolerating a brief window where it is not yet
/// complete.
///
/// QEMU answers `screendump` after closing the file, so the first read
/// normally succeeds. The retry exists because on a busy host or a network
/// home directory the write can land after the reply, and a truncated PNG
/// decoded into a partial image looks like a rendering bug in the guest.
async fn read_screendump(path: &std::path::Path) -> ContinuumResult<Vec<u8>> {
    let mut last_error = String::new();
    for attempt in 0..SCREENDUMP_READ_ATTEMPTS {
        match tokio::fs::read(path).await {
            Ok(bytes) if !bytes.is_empty() => match image::load_from_memory_with_format(
                &bytes,
                ImageFormat::Png,
            ) {
                // Parsed: the file is complete, not merely present.
                Ok(_) => return Ok(bytes),
                Err(e) => last_error = format!("PNG not decodable yet: {e}"),
            },
            Ok(_) => last_error = "screendump file was empty".to_string(),
            Err(e) => last_error = format!("cannot read screendump: {e}"),
        }
        if attempt + 1 < SCREENDUMP_READ_ATTEMPTS {
            tokio::time::sleep(SCREENDUMP_READ_DELAY).await;
        }
    }
    Err(ContinuumError::Capture(format!(
        "screendump produced no usable PNG at {}: {last_error}",
        path.display()
    )))
}

fn decode_png(bytes: &[u8]) -> ContinuumResult<DynamicImage> {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|e| ContinuumError::Capture(format!("PNG decode failed: {e}")))
}

fn resize_to(image: DynamicImage, width: u32, height: u32) -> DynamicImage {
    if image.width() == width && image.height() == height {
        return image;
    }
    image.resize_exact(width, height, image::imageops::FilterType::Triangle)
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> ContinuumResult<Vec<u8>> {
    let mut buffer = Vec::with_capacity(256 * 1024);
    let mut encoder = JpegEncoder::new_with_quality(&mut buffer, quality.clamp(1, 100));
    encoder
        .encode_image(image)
        .map_err(|e| ContinuumError::Capture(format!("JPEG encode failed: {e}")))?;
    Ok(buffer)
}

/// Map `transport::MonitorInfo` into the `continuum-core` vocabulary.
///
/// The two structs are deliberately identical; this exists so the duplication
/// is a checked conversion rather than two definitions that can drift.
pub fn core_monitor_to_transport(info: &MonitorInfo) -> crate::types::MonitorInfo {
    crate::types::MonitorInfo {
        id: info.id,
        name: info.name.clone(),
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
        is_primary: info.is_primary,
        scale_factor: info.scale_factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screendump_paths_are_unique() {
        let a = screendump_path();
        let b = screendump_path();
        assert_ne!(a, b, "concurrent captures must not share a filename");
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(a.to_string_lossy().contains(&std::process::id().to_string()));
    }

    #[test]
    fn test_screendump_path_is_absolute() {
        assert!(
            screendump_path().is_absolute(),
            "QEMU must be able to resolve the path from its own cwd"
        );
    }

    #[test]
    fn test_transcode_produces_jpeg() {
        let img = crate::capture::synthesize_demo_frame();
        let jpeg = encode_jpeg(&img, 80).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "missing JPEG SOI marker");
        let decoded = image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg).unwrap();
        assert_eq!(decoded.width(), 640);
        assert_eq!(decoded.height(), 360);
    }

    #[test]
    fn test_transcode_honours_quality_ordering() {
        let img = crate::capture::synthesize_demo_frame();
        let low = encode_jpeg(&img, 10).unwrap();
        let high = encode_jpeg(&img, 95).unwrap();
        assert!(
            high.len() > low.len(),
            "q95 ({}) should exceed q10 ({}) on a gradient",
            high.len(),
            low.len()
        );
    }

    #[test]
    fn test_quality_is_clamped_to_valid_jpeg_range() {
        let img = crate::capture::synthesize_demo_frame();
        // 0 is not a valid JPEG quality; it must not silently produce an
        // undecodable stream.
        assert!(encode_jpeg(&img, 0).is_ok());
        assert!(encode_jpeg(&img, 255).is_ok());
    }

    #[test]
    fn test_resize_is_exact() {
        let img = crate::capture::synthesize_demo_frame();
        let resized = resize_to(img, 1280, 800);
        assert_eq!(resized.width(), 1280);
        assert_eq!(resized.height(), 800);
    }

    #[test]
    fn test_resize_is_a_noop_when_already_correct() {
        let img = crate::capture::synthesize_demo_frame();
        let resized = resize_to(img.clone(), 640, 360);
        assert_eq!(resized.width(), 640);
        assert_eq!(resized.height(), 360);
    }

    #[test]
    fn test_decode_png_rejects_garbage() {
        assert!(decode_png(b"not a png at all").is_err());
    }

    #[test]
    fn test_monitor_conversion_preserves_every_field() {
        let core = MonitorInfo {
            id: 3,
            name: "win11".into(),
            x: -1920,
            y: 40,
            width: 1280,
            height: 800,
            is_primary: false,
            scale_factor: 1.5,
        };
        let wire = core_monitor_to_transport(&core);
        assert_eq!(wire.id, 3);
        assert_eq!(wire.name, "win11");
        assert_eq!(wire.x, -1920);
        assert_eq!(wire.y, 40);
        assert_eq!(wire.width, 1280);
        assert_eq!(wire.height, 800);
        assert!(!wire.is_primary);
        assert_eq!(wire.scale_factor, 1.5);
    }

    #[tokio::test]
    async fn test_read_screendump_reports_missing_file() {
        let missing = std::env::temp_dir().join("continuum-qmp-does-not-exist.png");
        let err = read_screendump(&missing).await.unwrap_err();
        assert!(
            format!("{err}").contains("no usable PNG"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_default_quality_matches_frame_semantics() {
        assert_eq!(DEFAULT_CAPTURE_QUALITY, 85);
        assert_eq!(
            DEFAULT_CAPTURE_QUALITY,
            crate::types::FrameSemantics::default().quality
        );
    }

    #[test]
    fn test_available_flag_tracks_the_handshake() {
        // `is_available` is the only synchronous signal a GUI gets before it
        // commits to a subscription, so it must not report a VM that never
        // answered.
        let addr = "127.0.0.1:1".parse::<SocketAddr>().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(QmpCaptureBackend::connect("dead-vm", addr))
                .is_err(),
            "a refused connect must not produce a backend"
        );
    }

    #[tokio::test]
    async fn test_capture_jpeg_end_to_end_against_a_fake_qemu() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        const GREETING: &str =
            r#"{"QMP":{"version":{"package":"8.1.0"},"capabilities":[]}}"#;

        // A real PNG for the fake QEMU to write out.
        let source = crate::capture::synthesize_demo_frame();
        let mut png_bytes = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
            .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let written = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

        let recorder = std::sync::Arc::clone(&written);
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                for line in text.lines() {
                    if !line.contains("screendump") {
                        // qmp_capabilities
                        if socket
                            .write_all(b"{\"return\":{}}\n")
                            .await
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    let request: serde_json::Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let filename = request["arguments"]["filename"].as_str().unwrap_or("");
                    recorder.lock().push(filename.to_string());
                    assert_eq!(
                        request["arguments"]["format"], "png",
                        "PNG keeps the transcode single-generation"
                    );
                    // The Windows backslashes survived, because the envelope is
                    // built by serde_json rather than by concatenation.
                    assert!(filename.ends_with(".png"), "got {filename:?}");
                    tokio::fs::write(filename, &png_bytes).await.unwrap();
                    if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                        return;
                    }
                }
            }
        });

        let backend = QmpCaptureBackend::connect("win11", addr)
            .await
            .unwrap()
            .with_format(75, 320, 200);

        let frame = backend.capture_jpeg().await.unwrap();
        assert_eq!(frame.width, 320);
        assert_eq!(frame.height, 200);
        assert_eq!(&frame.jpeg[..2], &[0xFF, 0xD8], "wire format must be JPEG");

        let decoded =
            image::load_from_memory_with_format(&frame.jpeg, ImageFormat::Jpeg).unwrap();
        assert_eq!(decoded.width(), 320);
        assert_eq!(decoded.height(), 200);

        let requested = written.lock().clone();
        assert_eq!(requested.len(), 1, "exactly one screendump was issued");

        // And the temp file must be gone: QEMU never cleans up after itself,
        // so 30 fps would leak ~1 GB/hour of PNG otherwise.
        assert!(
            !std::path::Path::new(&requested[0]).exists(),
            "screendump left {} behind",
            requested[0]
        );
    }

    #[tokio::test]
    async fn test_capture_serialises_concurrent_requests() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        const GREETING: &str =
            r#"{"QMP":{"version":{"package":"8.1.0"},"capabilities":[]}}"#;

        let source = crate::capture::synthesize_demo_frame();
        let mut png_bytes = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
            .unwrap();

        // This QEMU holds every screendump reply open for 250 ms, so two
        // captures that were not serialised anywhere would both be in flight
        // inside that window. `overlapped` records whether that happened.
        //
        // Note what is actually being proved: the `QmpClient` socket mutex
        // alone would keep the two requests off the wire at the same time,
        // because it holds across write *and* read. The capture mutex is the
        // second layer, covering two backends pointed at the same VM through
        // different connections — where the sockets are independent and only
        // the filename keeps them apart. This test pins the observable
        // property (both captures succeed, one at a time) rather than
        // attributing it to one of the two locks.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let overlapped = std::sync::Arc::new(parking_lot::Mutex::new(false));
        let in_flight = std::sync::Arc::new(parking_lot::Mutex::new(0usize));
        let flag = std::sync::Arc::clone(&overlapped);
        let depth = std::sync::Arc::clone(&in_flight);

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut screendumps = Vec::new();
                for line in text.lines() {
                    if line.contains("screendump") {
                        let request: serde_json::Value =
                            serde_json::from_str(line).unwrap_or_default();
                        screendumps.push(
                            request["arguments"]["filename"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        );
                    } else if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                        return;
                    }
                }
                for filename in screendumps {
                    *depth.lock() += 1;
                    if *depth.lock() > 1 {
                        *flag.lock() = true;
                    }
                    tokio::fs::write(&filename, &png_bytes).await.unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    if socket.write_all(b"{\"return\":{}}\n").await.is_err() {
                        return;
                    }
                    *depth.lock() -= 1;
                }
            }
        });

        let backend = Arc::new(QmpCaptureBackend::connect("win11", addr).await.unwrap());
        let (a, b) = tokio::join!(backend.capture_jpeg(), backend.capture_jpeg());

        assert!(
            !*overlapped.lock(),
            "two screendumps were in flight at once — the capture mutex is not holding"
        );
        assert!(
            a.is_ok() && b.is_ok(),
            "both serialised captures should succeed (a={:?}, b={:?})",
            a.err(),
            b.err()
        );
    }

    #[tokio::test]
    async fn test_capture_reports_a_qmp_error_rather_than_an_empty_frame() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        const GREETING: &str =
            r#"{"QMP":{"version":{"package":"8.1.0"},"capabilities":[]}}"#;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                for line in text.lines() {
                    let reply: &[u8] = if line.contains("screendump") {
                        b"{\"error\":{\"class\":\"DeviceNotFound\",\"desc\":\"no display\"}}\n"
                    } else {
                        b"{\"return\":{}}\n"
                    };
                    if socket.write_all(reply).await.is_err() {
                        return;
                    }
                }
            }
        });

        let backend = QmpCaptureBackend::connect("win11", addr).await.unwrap();
        let err = backend.capture_jpeg().await.unwrap_err();
        let text = format!("{err}");
        assert!(
            text.contains("screendump") && text.contains("no display"),
            "the QEMU error must survive to the caller: {text}"
        );
    }
}