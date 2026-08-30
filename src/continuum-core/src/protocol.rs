use crate::*;
///
/// Implement this to add support for new protocol versions
/// with backward-compatible negotiation.
pub trait ProtocolNegotiator: Send + Sync {
    /// Negotiate protocol version with a remote peer.
    fn negotiate(&mut self, peer_versions: &[ProtocolVersion]) -> ContinuumResult<ProtocolVersion>;

    /// Get the versions supported by this implementation.
    fn supported_versions(&self) -> &[ProtocolVersion];

    /// Check if a specific version is supported.
    fn supports_version(&self, version: &ProtocolVersion) -> bool;
}

/// A protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl ProtocolVersion {
    /// Create a new protocol version.
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if two versions are compatible.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        // Major version must match for compatibility
        self.major == other.major
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Current protocol version supported by this implementation.
pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0, 0);

/// Minimum protocol version supported by this implementation.
pub const MINIMUM_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0, 0);
