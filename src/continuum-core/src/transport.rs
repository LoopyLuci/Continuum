use crate::*;

/// Network transport trait.
///
/// Implement this to add support for new transports (WebRTC, SCTP, custom).
pub trait Transport: Send + Sync {
    /// Connect to a remote endpoint.
    fn connect(&mut self, addr: &str) -> ContinuumResult<Box<dyn TransportConnection>>;

    /// Accept an incoming connection.
    fn accept(&mut self) -> ContinuumResult<Box<dyn TransportConnection>>;

    /// Close the transport.
    fn close(&mut self);
}

/// A transport connection.
pub trait TransportConnection: Send + Sync {
    /// Open a bidirectional stream.
    fn open_bi(&mut self) -> ContinuumResult<Box<dyn BiStream>>;

    /// Accept a bidirectional stream.
    fn accept_bi(&mut self) -> ContinuumResult<Box<dyn BiStream>>;

    /// Close the connection.
    fn close(&mut self);

    /// Get connection statistics.
    fn stats(&self) -> ConnectionStats;
}

/// A bidirectional stream.
pub trait BiStream: Send + Sync {
    /// Send data.
    fn send(&mut self, data: &[u8]) -> ContinuumResult<()>;

    /// Receive data into a buffer.
    fn recv(&mut self, buf: &mut [u8]) -> ContinuumResult<usize>;

    /// Receive exact data.
    fn recv_exact(&mut self, buf: &mut [u8]) -> ContinuumResult<()>;

    /// Close the stream.
    fn close(&mut self);
}

/// Connection statistics.
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub rtt_ms: f32,
    pub packet_loss: f32,
}
