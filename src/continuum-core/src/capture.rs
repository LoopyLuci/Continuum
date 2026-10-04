use crate::*;

/// Implement this to add support for new capture methods
/// (Wayland, DXGI, Metal, QMP/QEMU, etc.).
///
/// The trait signatures here are the *normative* definition of a capture
/// backend; `continuum-transport::types` mirrors them for the wire protocol.
/// The two must stay field-for-field identical — `MonitorInfo` here carries
/// the same `x`/`y`/`scale_factor` the QUIC `ServerInfo` advertises — because
/// a mismatch here is invisible until a client is told a monitor is at the
/// wrong place. `continuum_transport::capture_qmp` provides the `From`
/// conversions in both directions; the crate dependency points transport ->
/// core, never the reverse, so core cannot name transport's types directly.
///
/// Two implementations exist and both are live:
///   * `continuum_transport::capture_qmp::QmpCaptureBackend` — a QEMU VM
///     framebuffer read over QMP `screendump`.
///   * `continuum_transport::capture::capture_monitor` — the physical
///     monitor path (`screenshots` on Windows, `scrap` elsewhere). This is
///     the default; QMP is selected per-VM.
pub trait CaptureBackend: Send + Sync {
    /// Enumerate available monitors.
    fn enumerate_monitors(&self) -> ContinuumResult<Vec<MonitorInfo>>;

    /// Capture a frame from a specific monitor.
    fn capture_frame(&mut self, monitor_id: u32) -> ContinuumResult<CapturedFrame>;

    /// Get the backend name.
    fn name(&self) -> &str;

    /// Check if the backend is available on the current platform.
    fn is_available(&self) -> bool;
}

/// Information about a monitor.
///
/// Mirrors `continuum_transport::types::MonitorInfo`. `x`/`y` exist because a
/// QMP-sourced VM display and a physical monitor can both be non-zero-origin,
/// and `scale_factor` because a Retina or 150%-scaled host display changes
/// what an absolute pointer coordinate means.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    pub scale_factor: f32,
}

/// A captured frame.
///
/// `data` is always encoded for the wire. The QUIC media path and the
/// WebSocket sidecar both carry JPEG, so backends transcode to JPEG rather
/// than handing out raw RGBA: a 1080p raw frame is 8 MB and a JPEG is ~150 KB.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub monitor_id: u32,
}

impl CapturedFrame {
    /// Build a frame stamped with the current wall clock, in microseconds.
    ///
    /// Microseconds because that is the resolution the QUIC media header
    /// carries; a millisecond timestamp quantises a 60 fps stream into 16
    /// indistinguishable buckets.
    pub fn new(width: u32, height: u32, data: Vec<u8>, monitor_id: u32) -> Self {
        Self {
            width,
            height,
            data,
            timestamp_us: now_micros(),
            monitor_id,
        }
    }

    /// True when `data` starts with a JPEG SOI marker.
    ///
    /// Checked at the boundary where frames leave a backend so a backend that
    /// silently produces raw pixels fails here rather than in a client that
    /// cannot decode the stream.
    pub fn is_jpeg(&self) -> bool {
        self.data.len() >= 2 && self.data[0] == 0xFF && self.data[1] == 0xD8
    }
}

/// Microseconds since the Unix epoch.
pub fn now_micros() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_captured_frame_new_stamps_timestamp() {
        let frame = CapturedFrame::new(1920, 1080, vec![0xFF, 0xD8, 0xFF], 0);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.monitor_id, 0);
        assert!(frame.timestamp_us > 0);
    }

    #[test]
    fn test_captured_frame_detects_jpeg() {
        assert!(CapturedFrame::new(1, 1, vec![0xFF, 0xD8, 0x00], 0).is_jpeg());
        assert!(!CapturedFrame::new(1, 1, vec![0x89, 0x50, 0x4E], 0).is_jpeg());
        assert!(!CapturedFrame::new(1, 1, vec![], 0).is_jpeg());
    }

    #[test]
    fn test_now_micros_is_monotonic_scale() {
        // Two calls a few microseconds apart must not go backwards; a
        // regressing frame clock breaks the encoder's keyframe interval.
        let a = now_micros();
        let b = now_micros();
        assert!(b >= a);
        assert!(a > 1_600_000_000_000_000, "should be a real epoch value");
    }
}