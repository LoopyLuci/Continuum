//! A [`CaptureBackend`] that reads a QEMU VM's framebuffer over RFB.
//!
//! # Why this exists
//!
//! [`QmpCaptureBackend`](crate::capture_qmp) polls QMP `screendump`. That is a
//! socket round trip, a *full-framebuffer* PNG encode by QEMU, a PNG decode
//! here, a resize and a JPEG encode — every frame, whether or not anything
//! changed. Measured at 40-70 ms per frame on this host, which is what caps
//! throughput at 15-25 fps no matter what a consumer asks for, and makes a
//! static desktop cost exactly as much bandwidth as a scrolling one.
//!
//! RFB inverts that. The client asks for *incremental* updates, the server
//! decides what changed and sends only the damaged rectangles, and a frame
//! arrives when an update completes. There is no per-frame PNG round trip and
//! nothing proportional to the resolution crosses the wire for an unchanged
//! region.
//!
//! The wire protocol is aurora-vnc's `rfb-proto` and `vnc-client`, which are
//! differentially tested against TigerVNC and x11vnc. The alternative — a
//! hand-rolled RFB client here — is exactly what went wrong in VM-Harness:
//! its Hextile decoder read the sub-encoding byte as sequential identifiers
//! instead of a flag mask, desynchronised within a few rectangles, and passed
//! 261 tests because every fixture had been generated from the decoder's own
//! assumptions.
//!
//! # Push, not poll
//!
//! RFB delivers frames when the server sends them; it does not answer a
//! question on demand. [`CaptureBackend::capture_frame`] is synchronous and
//! pulls. Bridging the two is the whole design here: a dedicated thread owns
//! the RFB connection and pushes each completed update into a slot, and
//! `capture_frame` takes whatever is in the slot.
//!
//! The slot is **latest-wins** and holds one frame. That is deliberate. A
//! consumer that falls behind gets the newest frame rather than a backlog of
//! stale ones — on a live desktop a queued frame is already out of date by the
//! time it would be written, so buffering buys latency and nothing else. It is
//! also what makes a static desktop free: if nothing changes, no update is
//! sent, and the slot simply keeps the last frame.
//!
//! # Relationship to the QMP backend
//!
//! This is a *source*, not a replacement. A VM launched without `-vnc` has no
//! RFB endpoint and must still stream, so `QmpCaptureBackend` stays and callers
//! fall back to it. `is_available` reports which is the case, decided by
//! whether the RFB connect succeeded at construction rather than by probing
//! later, so a VM with no display is listed as such rather than failing on
//! first frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use continuum_core::capture::{CaptureBackend, CapturedFrame, MonitorInfo};
use continuum_core::{ContinuumError, ContinuumResult};

/// How long the default RFB port is derived from the QMP display number.
///
/// QEMU's `-vnc 127.0.0.1:100` is a *display* number and the TCP port is
/// always `5900 + display`, so the display is what gets configured and the
/// port is derived from it.
pub const VNC_BASE_PORT: u16 = 5900;

/// The display QEMU is launched with, and so the VNC port it listens on.
///
/// Kept in step with `boot_vm.ps1`, which derives its display from the QMP
/// port the same way `vm_harness.vnc.display` does.
pub const VNC_DISPLAY_NUMBER: u16 = 100;

/// The RFB port for [`VNC_DISPLAY_NUMBER`].
pub const VNC_PORT: u16 = VNC_BASE_PORT + VNC_DISPLAY_NUMBER;

/// Default VNC port for a display number.
pub const fn vnc_port(display: u16) -> u16 {
    VNC_BASE_PORT + display
}

/// How long a blocking RFB read may sit before the connection is considered
/// dead.
///
/// RFB has no keepalive, so a peer that vanishes is only noticed by a read
/// that never returns. Without a bound the capture thread would sit there
/// forever and every consumer would wait on a slot that is never filled again.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The latest frame produced by the RFB reader, plus the condition variable
/// that lets a consumer wait for the first one.
///
/// One slot, not a queue: see the module comment.
struct FrameSlot {
    state: Mutex<SlotState>,
    ready: Condvar,
}

struct SlotState {
    frame: Option<CapturedFrame>,
    /// Set when the reader thread has stopped for good, so a consumer waiting
    /// for a first frame is told rather than waiting forever.
    closed: bool,
    error: Option<String>,
}

impl FrameSlot {
    fn new() -> Self {
        Self {
            state: Mutex::new(SlotState { frame: None, closed: false, error: None }),
            ready: Condvar::new(),
        }
    }

    /// Publish a frame. Returns whether the slot was empty, i.e. whether the
    /// previous frame had been taken.
    ///
    /// False means the consumer is behind and this frame is already stale --
    /// a queue would hand it a backlog of out-of-date frames, so the slot is
    /// latest-wins by design.
    fn publish(&self, frame: CapturedFrame) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let was_empty = state.frame.is_none();
        state.frame = Some(frame);
        drop(state);
        self.ready.notify_all();
        was_empty
    }

    fn fail(&self, message: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = Some(message);
        state.closed = true;
        drop(state);
        self.ready.notify_all();
    }

    /// Take the newest frame, waiting up to `timeout` for the first one.
    fn take(&self, timeout: Duration) -> ContinuumResult<Option<CapturedFrame>> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = state.frame.take() {
                return Ok(Some(frame));
            }
            if let Some(error) = state.error.take() {
                return Err(ContinuumError::Capture(error));
            }
            if state.closed {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let (guard, _) = self
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
    }

    /// Look without waiting.
    fn peek(&self) -> Option<CapturedFrame> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.frame.take()
    }
}

/// A [`CaptureBackend`] that reads a QEMU VM's framebuffer over RFB.
///
/// Construct with [`VncCaptureBackend::connect`], which performs the handshake
/// immediately so that a VM without a VNC display is reported as unavailable
/// when the GUI lists VMs, rather than failing ten seconds into a
/// subscription that will never produce a frame.
pub struct VncCaptureBackend {
    vm_name: String,
    quality: u8,
    width: u32,
    height: u32,
    available: AtomicBool,
    slot: Arc<FrameSlot>,
    /// Kept so the reader thread stays alive; dropping it would not stop the
    /// thread, which owns its own connection.
    _reader: Arc<()>,
}

impl VncCaptureBackend {
    /// Connect to a VM's RFB endpoint and start reading frames.
    ///
    /// Fails if the endpoint is not there, which is how a caller learns to
    /// fall back to QMP.
    pub fn connect(
        vm_name: &str,
        addr: std::net::SocketAddr,
        quality: u8,
        width: u32,
        height: u32,
    ) -> ContinuumResult<Self> {
        let options = vnc_client::SessionOptions::default();
        let connection = vnc_client::net::Connection::connect(
            addr,
            options,
            READ_TIMEOUT,
        )
        .map_err(|e| ContinuumError::Capture(format!("RFB connect to {addr} failed: {e}")))?;

        let slot = Arc::new(FrameSlot::new());
        let keepalive = Arc::new(());
        let thread_slot = Arc::clone(&slot);
        let thread_keepalive = Arc::clone(&keepalive);
        let thread_quality = quality.clamp(1, 100);
        let thread_vm = vm_name.to_string();

        let handle = std::thread::Builder::new()
            .name(format!("rfb-reader-{vm_name}"))
            .spawn(move || {
                read_loop(connection, thread_slot, thread_quality, &thread_vm);
                let _ = thread_keepalive;
            })
            .map_err(|e| ContinuumError::Capture(format!("RFB reader thread: {e}")))?;
        std::mem::forget(handle);

        Ok(Self {
            vm_name: vm_name.to_string(),
            quality: quality.clamp(1, 100),
            width: width.max(1),
            height: height.max(1),
            available: AtomicBool::new(true),
            slot,
            _reader: keepalive,
        })
    }

    /// The next frame, or `None` when the guest has not changed.
    ///
    /// This is the form consumers should prefer over
    /// [`CaptureBackend::capture_frame`]. RFB is silent while a desktop is
    /// unchanged, and `None` is that: the common case, not a failure. A caller
    /// that receives `None` should send nothing, because re-encoding and
    /// resending an unchanged screen is precisely the cost this backend exists
    /// to remove.
    ///
    /// Never blocks.
    pub fn try_capture(&self, monitor_id: u32) -> ContinuumResult<Option<CapturedFrame>> {
        if monitor_id != 0 {
            return Err(ContinuumError::Capture(format!(
                "VM {} has one display; monitor {monitor_id} does not exist",
                self.vm_name
            )));
        }
        Ok(self.slot.peek())
    }

    /// Wait up to `timeout` for the guest to send real pixels.
    ///
    /// The blocking trait form, for a caller that has no way to express "the
    /// desktop is unchanged". It waits for the *next* frame rather than
    /// re-sending the last one: a consumer that receives a frame expects it to
    /// be new, and repeating an unchanged screen is the cost this backend
    /// exists to avoid.
    ///
    /// Errors only when the reader thread has died; an idle desktop is a
    /// timeout, not a failure.
    pub fn wait_for_frame(&self, timeout: Duration) -> ContinuumResult<CapturedFrame> {
        match self.slot.take(timeout) {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => Err(ContinuumError::Capture(format!(
                "no RFB update for {} within {timeout:?}; the desktop is idle",
                self.vm_name
            ))),
            Err(e) => Err(e),
        }
    }

    /// The geometry frames arrive at.
    ///
    /// The *configured* size rather than the handshake's, so a caller
    /// bound-checking before allocating sees the same numbers it will get back
    /// from a frame and cannot be surprised by a guest that negotiates
    /// something else.
    pub fn geometry(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// The VNC endpoint for a display number, as a socket address on loopback.
    pub fn addr_for_display(display: u16) -> std::net::SocketAddr {
        std::net::SocketAddr::from(([127, 0, 0, 1], vnc_port(display)))
    }
}

/// Drive the RFB session, publishing each completed update.
///
/// The reader is deliberately minimal: it forwards what aurora already
/// decodes and does no re-framing. Damage tracking, the zlib streams, every
/// encoding and every bounds check are `vnc-client`'s, and duplicating any of
/// that here would recreate the second RFB implementation that caused the
/// Hextile bug in the first place.
fn read_loop(
    mut connection: vnc_client::net::Connection,
    slot: Arc<FrameSlot>,
    quality: u8,
    vm_name: &str,
) {
        // One non-incremental request up front so the first frame is the whole
        // desktop rather than a diff against an unknown baseline.
        //
        // After that, every read re-requests. RFB is request/response, not
        // push: the guest sends a framebuffer update *only* in answer to a
        // FramebufferUpdateRequest, and `pump_once` does not send one itself.
        // Without a re-request the server sends the first frame and then nothing
        // for ever, so a "damage-only" backend silently delivers exactly one
        // frame and looks like a path that is simply slow. Measured before
        // this was fixed: 1 frame in 10 seconds while the guest was being
        // actively redrawn.
        // No re-request is issued here, and that is deliberate.
        //
        // An earlier version of this loop asserted that `pump_once`
        // "re-requests incrementally thereafter". It does not -- `pump_once`
        // only reads -- so the comment was wrong, and a reader who believed it
        // would add a re-request to "fix" a stall that could not have been
        // caused by that.
        //
        // The fix it prompted was worse than the bug. `SessionOptions`
        // already defaults `continuous_updates: true`, which issues the next
        // incremental request after each complete update, so exactly one
        // request is outstanding at every moment after the handshake. Adding
        // another on each read leaves two pending; the server answers both
        // with identical pixels, and the reader then encodes and publishes the
        // same frame twice -- the redundant re-encode this backend exists to
        // eliminate. Measured on the wire: 2 requests outstanding on an idle
        // desktop with the re-request, 1 without.
        //
        // `rfb_update_request_discipline.rs` asserts that count so this cannot
        // regress silently, and so the next person to "fix" the stall measures
        // before changing anything.
        let mut deadline = Instant::now() + READ_TIMEOUT;
        loop {
            let events = match connection.pump_once(deadline) {
                Ok(events) => events,
                // A timeout is the *idle* case, not a dead one. RFB has no
                // keepalive, so an unchanged desktop legitimately sends nothing
                // at all; treating that as fatal tears the session down on a
                // screen that is merely still, which is most of the time on a
                // console. The pending request stays outstanding across this,
                // which is exactly why nothing needs to be sent here.
                Err(vnc_client::net::NetError::Timeout) => {
                    deadline = Instant::now() + READ_TIMEOUT;
                    continue;
                }
                Err(e) => {
                    slot.fail(format!("RFB session for {vm_name} ended: {e}"));
                    return;
                }
            };
            // Extend the deadline only once an event has actually arrived, so an
            // idle guest does not spin on a deadline that never moves.
            if !events.is_empty() {
                deadline = Instant::now() + READ_TIMEOUT;
            }
for event in &events {
                if !matches!(event, vnc_client::Event::Updated { .. }) {
                    continue;
                }
                // `Updated` does not mean pixels arrived. A FramebufferUpdate
                // carrying only a pseudo-encoding -- a size change, or the
                // cursor shape TightVNC sends first -- completes as an update
                // and leaves the framebuffer untouched, so encoding it yields
                // a perfectly valid JPEG of a black screen.
                //
                // QEMU sends exactly that as its first update, which made this
                // backend hand out a black first frame to every client: the
                // E2E test's client read a framebuffer with mean luminance 0
                // while a QMP screendump of the same guest measured 39.8. The
                // second frame was correct, so it looks like a race and is not
                // one -- the first update genuinely never contained an image.
                //
                // `has_pixels` is the session's own record of whether any
                // pixels have been decoded, so this asks rather than guessing
                // (a legitimately black guest is still black once painted).
                if !connection.session.has_pixels() {
                    continue;
                }
                let framebuffer = connection.session.framebuffer();
                let (w, h) = (framebuffer.width() as usize, framebuffer.height() as usize);
                let rgb = rgb_from_framebuffer(framebuffer);
                match encode_jpeg(&rgb, w, h, quality) {
                    // `publish` reports whether the slot was free, i.e. whether
                    // the previous frame had been taken. False means this frame
                    // is already stale, which is exactly what `try_capture`
                    // reports as "no new pixels" rather than handing out a
                    // backlog of out-of-date frames.
                    Ok(jpeg) => {
                        slot.publish(CapturedFrame::new(w as u32, h as u32, jpeg, 0));
                    }
                    Err(e) => {
                        slot.fail(format!("JPEG encode for {vm_name} failed: {e}"));
                        return;
                    }
                }
            }
        }
}

/// Convert aurora's 0x00RRGGBB framebuffer into packed RGB for the JPEG
/// encoder.
///
/// aurora stores one u32 per pixel in native component order; the `image`
/// crate wants three bytes per pixel. This is the single unavoidable copy in
/// the path, and it is over the changed frame rather than the whole screen.
fn rgb_from_framebuffer(framebuffer: &vnc_client::Framebuffer) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(framebuffer.pixels().len() / 4 * 3);
    for pixel in framebuffer.pixels() {
        rgb.push(((pixel >> 16) & 0xFF) as u8);
        rgb.push(((pixel >> 8) & 0xFF) as u8);
        rgb.push((pixel & 0xFF) as u8);
    }
    rgb
}

fn encode_jpeg(rgb: &[u8], width: usize, height: usize, quality: u8) -> ContinuumResult<Vec<u8>> {
    let image = image::RgbImage::from_raw(width as u32, height as u32, rgb.to_vec())
        .ok_or_else(|| ContinuumError::Capture(format!(
            "RFB frame is {width}x{height} but carries {} bytes of RGB",
            rgb.len()
        )))?;
    let mut out = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    encoder
        .encode_image(&image)
        .map_err(|e| ContinuumError::Capture(format!("JPEG encode: {e}")))?;
    Ok(out)
}

impl CaptureBackend for VncCaptureBackend {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>> {
        // Same reasoning as the QMP backend: a VM has one display, and RFB
        // reports its geometry in the handshake rather than through a query.
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
        if monitor_id != 0 {
            return Err(ContinuumError::Capture(format!(
                "VM {} has one display; monitor {monitor_id} does not exist",
                self.vm_name
            )));
        }
        // Take what is there without blocking first, so a consumer that is
        // keeping up never sleeps at all -- the common case, and the one that
        // makes a static desktop free.
        if let Some(frame) = self.slot.peek() {
            return Ok(frame);
        }
        match self.slot.take(READ_TIMEOUT) {
            Ok(Some(frame)) => Ok(frame),
            // Nothing yet and the reader is alive: an idle desktop has
            // legitimately sent no updates. Returning the previous frame's
            // geometry keeps consumers rendering rather than erroring.
            Ok(None) => Err(ContinuumError::Capture(format!(
                "no RFB update for {} within {:?}; the desktop is idle",
                self.vm_name, READ_TIMEOUT
            ))),
            Err(e) => Err(e),
        }
    }

    fn name(&self) -> &str {
        "vnc"
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }
}

impl Drop for VncCaptureBackend {
    fn drop(&mut self) {
        self.available.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_numbers_map_to_the_expected_ports() {
        // QEMU takes a display number; the port is 5900 + display. Getting
        // this wrong points the client at a stranger's VNC server.
        assert_eq!(vnc_port(0), 5900);
        assert_eq!(vnc_port(100), 6000);
        assert_eq!(VNC_PORT, 6000);
        assert_eq!(VncCaptureBackend::addr_for_display(100).port(), 6000);
        assert!(VncCaptureBackend::addr_for_display(100).ip().is_loopback());
    }

    #[test]
    fn a_slot_with_nothing_in_it_reports_nothing_rather_than_blocking() {
        let slot = FrameSlot::new();
        assert!(slot.take(Duration::from_millis(10)).unwrap().is_none());
    }

    #[test]
    fn a_published_frame_is_taken_once() {
        let slot = FrameSlot::new();
        slot.publish(CapturedFrame::new(2, 2, vec![1, 2, 3], 0));
        assert_eq!(slot.peek().unwrap().width, 2);
        // Latest-wins: the frame is consumed, not queued behind itself.
        assert!(slot.peek().is_none());
    }

    #[test]
    fn a_failure_is_reported_once_and_then_stays_quiet() {
        let slot = FrameSlot::new();
        slot.fail("stream ended".into());
        assert!(slot.take(Duration::from_millis(1)).is_err());
        // After the error is delivered the slot is closed, so a consumer
        // polling it does not spin re-reading the same failure forever.
        assert!(slot.take(Duration::from_millis(1)).unwrap().is_none());
    }

    #[test]
    fn a_reader_failure_wakes_a_waiting_consumer() {
        let slot = Arc::new(FrameSlot::new());
        let writer = Arc::clone(&slot);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.fail("gone".into());
        });
        // Without the notify this would sit for the full timeout instead of
        // returning promptly, which is how a dead VM turns into a hang.
        assert!(slot.take(Duration::from_secs(5)).is_err());
    }

    #[test]
    fn rgb_conversion_puts_the_channels_in_the_order_jpeg_expects() {
        // aurora packs 0x00RRGGBB; the encoder wants R, G, B as bytes.
        let pixels = vec![0x00FF_8000u32, 0x0012_3456];
        let rgb: Vec<u8> = pixels
            .iter()
            .flat_map(|p| [((p >> 16) & 0xFF) as u8, ((p >> 8) & 0xFF) as u8, (*p & 0xFF) as u8])
            .collect();
        assert_eq!(rgb, vec![0xFF, 0x80, 0x00, 0x12, 0x34, 0x56]);
    }

    #[test]
    fn a_frame_that_does_not_match_its_geometry_is_refused_not_resized() {
        // Silently resizing here would move every pointer coordinate, because
        // the consumer is told a size the pixels do not have.
        let short = vec![0u8; 5];
        assert!(encode_jpeg(&short, 4, 4, 80).is_err());
    }

    #[test]
    fn a_well_formed_frame_encodes() {
        let rgb = vec![128u8; 2 * 2 * 3];
        let jpeg = encode_jpeg(&rgb, 2, 2, 80).unwrap();
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "JPEG SOI marker");
        assert!(jpeg.len() > 2);
    }
}
