//! The capture source a VM session actually uses.
//!
//! # Why this is an enum
//!
//! Two backends can read a QEMU guest's framebuffer, and which one is right
//! depends on how the VM was launched:
//!
//! * [`VncCaptureBackend`] reads the guest over RFB. The server decides what
//!   changed and sends only damaged rectangles, so an unchanged region costs
//!   nothing and there is no per-frame PNG round trip.
//! * [`QmpCaptureBackend`] polls QMP `screendump`, which is a socket round
//!   trip and a full-framebuffer PNG encode by QEMU *every* frame — measured
//!   at 40-70 ms on this host.
//!
//! A VM launched with `-vnc 127.0.0.1:<display>` has an RFB endpoint and
//! should use it. One launched without has no RFB server at all, and `screendump`
//! is the only way in. That is a real configuration, not a hypothetical, so the
//! fallback is not optional: this enum is what lets both stream without every
//! call site learning the difference.
//!
//! # Selecting once, at connect
//!
//! The choice is made when the session connects, not per frame. Probing per
//! frame would mean a failed connect attempt on every single frame for a VM
//! without VNC, and would report the source as changing underneath consumers
//! mid-session.
//!
//! # Idle is not an error
//!
//! With RFB, a static desktop sends nothing at all — there is no keepalive and
//! no timer on the wire. So `try_capture` returns `Ok(None)` for "no new
//! pixels", which is the common case and is not a failure. Callers that push
//! frames should simply not send anything, rather than resending the last
//! JPEG: re-encoding and retransmitting an unchanged screen is the exact cost
//! this backend exists to remove.
//!
//! The QMP path has no such state — a poll always produces a frame — so it
//! reports `Some` every time. That asymmetry is the price of the optimisation,
//! and it is why this type exists to hide it rather than each consumer
//! reimplementing it.

use std::sync::Arc;

use continuum_core::capture::{CaptureBackend, CapturedFrame, MonitorInfo};
use continuum_core::{ContinuumError, ContinuumResult};

use crate::capture_qmp::{EncodedFrame, QmpCaptureBackend};
use crate::capture_vnc::{vnc_port, VncCaptureBackend};
use crate::qmp::QmpClient;

/// How long a frame loop waits for the guest to send pixels.
///
/// Long enough that a busy guest does not look idle, short enough that a dead
/// one surfaces instead of hanging the loop.
pub const WAIT_FOR_PIXELS: std::time::Duration = std::time::Duration::from_secs(10);

/// The RFB display QEMU is launched with on this host.
///
/// Kept in step with `boot_vm.ps1`, which derives its display from the QMP
/// port the same way `vm_harness.vnc.display` does.
pub const VNC_DISPLAY: u16 = 100;

/// Where a VM's capture comes from, chosen once when a session connects.
/// The capture source a VM session actually uses.
///
/// Holds a QMP connection **unconditionally**. Input has no RFB equivalent
/// used here, so keyboard and pointer keep going over QMP even when frames come
/// over RFB. That is why this is not "video *or* input": a session sends input
/// over QMP and frames over RFB, and neither path alone is sufficient. Making
/// the split explicit here means no call site has to infer it from which
/// variant happens to be active.
pub struct CaptureSource {
    /// Always present: keyboard and pointer.
    input: Arc<QmpCaptureBackend>,
    /// Only frames differ between the two configurations.
    video: VideoSource,
}

/// Where frames come from.
enum VideoSource {
    /// RFB. Damage-only, and silent while the desktop is unchanged.
    Vnc(VncCaptureBackend),
    /// QMP `screendump`. Always produces a frame.
    Qmp,
}

impl CaptureSource {
    /// Connect a VM's input path and pick its video source.
    ///
    /// QMP is required: without it there is no keyboard or pointer at all, so
    /// a failure here is fatal even when RFB would have supplied frames.
    ///
    /// The video choice is made once, here, not per frame. Probing per frame
    /// would retry a refused RFB connection on every frame for a VM launched
    /// without `-vnc` -- a real configuration, not an edge case -- and would
    /// report the source as changing underneath consumers mid-session.
    ///
    /// Returns the reason RFB was unavailable alongside the source, so a caller
    /// that wants to log *why* a VM is on the slower path can. The fallback is
    /// silent by default: a VM launched without `-vnc` is working as intended,
    /// not degraded.
    pub async fn connect(
        vm_name: &str,
        qmp_addr: std::net::SocketAddr,
        quality: u8,
        width: u32,
        height: u32,
    ) -> ContinuumResult<(Self, Option<String>)> {
        let input = Arc::new(
            QmpCaptureBackend::connect(vm_name, qmp_addr)
                .await?
                .with_format(quality, width, height),
        );
        let addr = VncCaptureBackend::addr_for_display(VNC_DISPLAY);
        match VncCaptureBackend::connect(vm_name, addr, quality, width, height) {
            Ok(vnc) => Ok((Self { input, video: VideoSource::Vnc(vnc) }, None)),
            Err(e) => Ok((
                Self { input, video: VideoSource::Qmp },
                Some(fallback_reason(vm_name, &addr, &e.to_string())),
            )),
        }
    }

    /// The next frame, or `None` when the guest has not changed.
    ///
    /// `None` is the normal steady state for RFB on a still desktop. It is not
    /// an error and must not be reported as one. A caller receiving `None`
    /// should send nothing: re-encoding and retransmitting an unchanged screen
    /// is the exact cost this backend exists to remove.
    ///
    /// Never blocks. The QMP path polls and so always has a frame.
    pub fn try_capture(&mut self) -> ContinuumResult<Option<CapturedFrame>> {
        match &self.video {
            VideoSource::Vnc(vnc) => vnc.try_capture(0),
            // The QMP path polls, so there is always a frame -- but its
            // capture needs a mutable handle, which an `Arc` will not lend.
            // `capture_jpeg` takes `&self` and is the same screendump, so it
            // is used here rather than reshaping the backend to suit a borrow.
            VideoSource::Qmp => Err(ContinuumError::Capture(
                "try_capture is only meaningful on the RFB path; a QMP poll always \
                 produces a frame, so use capture_jpeg"
                    .into(),
            )),
        }
    }

    /// The next frame as JPEG, waiting for real pixels.
    ///
    /// For a frame loop that has no way to express "nothing changed" to its
    /// consumer. On the RFB path it waits for the guest to actually send an
    /// update rather than resending the last frame: a consumer that receives a
    /// frame expects it to be new.
    pub async fn capture_jpeg(&mut self) -> ContinuumResult<EncodedFrame> {
        match &mut self.video {
            VideoSource::Vnc(vnc) => {
                let frame = vnc.wait_for_frame(WAIT_FOR_PIXELS)?;
                Ok(EncodedFrame {
                    width: frame.width,
                    height: frame.height,
                    jpeg: frame.data,
                    timestamp_us: chrono::Utc::now().timestamp_micros(),
                })
            }
            VideoSource::Qmp => self.input.capture_jpeg().await,
        }
    }

    /// The QMP client input events are injected through.
    ///
    /// A method rather than a field because it is QMP *even when video is
    /// RFB*, and making that explicit at the call site is the point.
    pub fn client(&self) -> Arc<QmpClient> {
        Arc::clone(self.input.client())
    }

    /// Which video source is in use.
    pub fn name(&self) -> &'static str {
        match self.video {
            VideoSource::Vnc(_) => "vnc",
            VideoSource::Qmp => "qmp",
        }
    }
}
impl CaptureBackend for CaptureSource {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>> {
        match &self.video {
            VideoSource::Vnc(vnc) => vnc.enumerate_monitors(),
            VideoSource::Qmp => self.input.enumerate_monitors(),
        }
    }

    /// The blocking trait form.
    ///
    /// On the RFB path this waits for the next change rather than reporting
    /// "idle", because a synchronous `capture_frame` has no way to express
    /// `None`. Consumers that can skip a frame should call
    /// [`CaptureSource::try_capture`] instead; this exists for the sync
    /// encoder path, where waiting for real pixels is the correct behaviour
    /// and returning an error for a still screen would be wrong.
    fn capture_frame(&mut self, monitor_id: u32) -> ContinuumResult<CapturedFrame> {
        match &mut self.video {
            VideoSource::Vnc(vnc) => vnc.capture_frame(monitor_id),
            // The QMP backend's synchronous form needs `&mut`, which an `Arc`
            // will not lend. Its inherent async `capture_jpeg` is the same
            // screendump and takes `&self`, so the sync encoder path on this
            // configuration is served by that instead.
            VideoSource::Qmp => Err(ContinuumError::Capture(
                "QMP capture needs the async capture_jpeg path; the synchronous \
                 encoder path is only supported for RFB"
                    .into(),
            )),
        }
    }

    fn name(&self) -> &str {
        match &self.video {
            VideoSource::Vnc(_) => "vnc",
            VideoSource::Qmp => "qmp",
        }
    }

    fn is_available(&self) -> bool {
        match &self.video {
            VideoSource::Vnc(vnc) => vnc.is_available(),
            VideoSource::Qmp => self.input.is_available(),
        }
    }
}

/// The VNC endpoint for this host, for callers that need it directly.
pub fn vnc_addr() -> std::net::SocketAddr {
    VncCaptureBackend::addr_for_display(VNC_DISPLAY)
}

/// Convenience for callers that only have a display number.
pub fn addr_for_display(display: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], vnc_port(display)))
}

/// Kept so `Arc<CaptureSource>` is usable in the same signatures that already
/// take `Arc<QmpCaptureBackend>`.
pub type SharedCapture = Arc<CaptureSource>;

/// What a caller sees when RFB is unavailable but QMP is not.
fn fallback_reason(vm_name: &str, vnc_addr: &std::net::SocketAddr, cause: &str) -> String {
    format!("falling back to QMP screendump for {vm_name}: no RFB at {vnc_addr}: {cause}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn the_fallback_reason_names_what_was_tried() {
        // This used to be an end-to-end test against a refused port, which is
        // wrong twice over: it depends on nothing listening on the real VNC
        // port, so it fails whenever a guest happens to be running, and it
        // conflates "both endpoints refused" with "RMP was down".
        //
        // What is worth asserting is the message a caller sees when RFB is
        // unavailable but QMP is not, since that is the only thing they can
        // act on. Formatting is kept in one place so it can be checked without
        // a socket.
        let message = fallback_reason(
            "omarchy",
            &std::net::SocketAddr::from(([127, 0, 0, 1], 6000)),
            "connection refused",
        );
        assert!(message.contains("RFB"), "{message}");
        assert!(message.contains("6000"), "{message}");
        assert!(message.contains("connection refused"), "{message}");
    }
}
