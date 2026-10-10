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

use crate::capture_qmp::QmpCaptureBackend;
use crate::capture_vnc::{vnc_port, VncCaptureBackend};

/// The RFB display QEMU is launched with on this host.
///
/// Kept in step with `boot_vm.ps1`, which derives its display from the QMP
/// port the same way `vm_harness.vnc.display` does.
pub const VNC_DISPLAY: u16 = 100;

/// Where a VM's capture comes from, chosen once when a session connects.
pub enum CaptureSource {
    /// RFB. Damage-only, and silent while the desktop is unchanged.
    Vnc(VncCaptureBackend),
    /// QMP `screendump`. Always produces a frame.
    Qmp(QmpCaptureBackend),
}

impl CaptureSource {
    /// Connect to a VM's capture source, preferring RFB and falling back to QMP.
    ///
    /// Returns the reason RFB was unavailable alongside the working source, so
    /// a caller that wants to log *why* a VM is on the slower path can. The
    /// fallback is silent by default: most sessions never ask, and a VM
    /// launched without `-vnc` is working as intended, not degraded.
    pub async fn connect(
        vm_name: &str,
        qmp_addr: std::net::SocketAddr,
        quality: u8,
        width: u32,
        height: u32,
    ) -> ContinuumResult<(Self, Option<String>)> {
        let addr = VncCaptureBackend::addr_for_display(VNC_DISPLAY);
        let vnc_error = match VncCaptureBackend::connect(vm_name, addr, quality, width, height) {
            Ok(backend) => return Ok((Self::Vnc(backend), None)),
            Err(e) => e.to_string(),
        };
        let backend = QmpCaptureBackend::connect(vm_name, qmp_addr).await.map_err(|e| {
            ContinuumError::Capture(both_failed(vm_name, &addr, &qmp_addr, &e.to_string()))
        })?;
        Ok((
            Self::Qmp(backend.with_format(quality, width, height)),
            Some(fallback_reason(vm_name, &addr, &vnc_error)),
        ))
    }

    /// Which source is in use.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Vnc(_) => "vnc",
            Self::Qmp(_) => "qmp",
        }
    }

    /// The next frame, or `None` when the guest has not changed.
    ///
    /// `None` is the normal steady state for RFB on a still desktop. It is
    /// not an error and must not be reported as one.
    pub fn try_capture(&mut self) -> ContinuumResult<Option<CapturedFrame>> {
        match self {
            Self::Vnc(vnc) => vnc.try_capture(0),
            Self::Qmp(qmp) => qmp.capture_frame(0).map(Some),
        }
    }
}

impl CaptureBackend for CaptureSource {
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>> {
        match self {
            Self::Vnc(vnc) => vnc.enumerate_monitors(),
            Self::Qmp(qmp) => qmp.enumerate_monitors(),
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
        match self {
            Self::Vnc(vnc) => vnc.capture_frame(monitor_id),
            Self::Qmp(qmp) => qmp.capture_frame(monitor_id),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Vnc(_) => "vnc",
            Self::Qmp(_) => "qmp",
        }
    }

    fn is_available(&self) -> bool {
        match self {
            Self::Vnc(vnc) => vnc.is_available(),
            Self::Qmp(qmp) => qmp.is_available(),
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

/// What a caller sees when neither source is reachable.
fn both_failed(
    vm_name: &str,
    vnc_addr: &std::net::SocketAddr,
    qmp_addr: &std::net::SocketAddr,
    cause: &str,
) -> String {
    format!(
        "neither RFB at {vnc_addr} nor QMP at {qmp_addr} is reachable for {vm_name}: {cause}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_is_loopback_and_the_expected_port() {
        let addr = vnc_addr();
        assert!(addr.ip().is_loopback(), "RFB must never be exposed off-host");
        assert_eq!(addr.port(), vnc_port(VNC_DISPLAY));
        assert_eq!(addr_for_display(100), addr);
    }

    #[test]
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

    #[test]
    fn both_endpoints_failing_says_so_and_names_each() {
        let message = both_failed(
            "omarchy",
            &std::net::SocketAddr::from(([127, 0, 0, 1], 6000)),
            &std::net::SocketAddr::from(([127, 0, 0, 1], 4444)),
            "connection refused",
        );
        assert!(message.contains("RFB"), "{message}");
        assert!(message.contains("QMP"), "{message}");
        assert!(message.contains("omarchy"), "{message}");
    }
}
