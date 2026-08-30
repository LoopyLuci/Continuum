use crate::*;
///
/// Implement this to add support for new capture methods
/// (Wayland, DXGI, Metal, etc.).
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
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// A captured frame.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub monitor_id: u32,
}
