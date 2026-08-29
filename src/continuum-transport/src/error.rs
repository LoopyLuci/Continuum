use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContinuumError {
    // --- Transport errors ---
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Connection timed out: {0}")]
    ConnectionTimeout(String),
    #[error("Connection closed unexpectedly: {0}")]
    ConnectionClosed(String),
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Invalid frame: {reason} (size: {size}, max: {max})")]
    InvalidFrame {
        reason: String,
        size: usize,
        max: usize,
    },
    #[error("Protocol violation: {0}")]
    ProtocolViolation(String),
    #[error("ALPN mismatch: expected {expected}, got {got}")]
    AlpnMismatch { expected: String, got: String },

    // --- Security errors ---
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Pairing rejected: {0}")]
    PairingRejected(String),
    #[error("Rate limited: retry after {retry_after}s")]
    RateLimited { retry_after: u64 },
    #[error("E2E encryption error: {0}")]
    E2EError(String),
    #[error("Certificate verification failed: {0}")]
    CertVerificationFailed(String),

    // --- Media pipeline errors ---
    #[error("Capture failed: {0}")]
    CaptureFailed(String),
    #[error("Monitor not found: {0}")]
    MonitorNotFound(String),
    #[error("Encode failed: {backend} - {reason}")]
    EncodeFailed { backend: String, reason: String },
    #[error("Decode failed: {0}")]
    DecodeFailed(String),
    #[error("Codec not supported: {0}")]
    CodecNotSupported(String),

    // --- Network errors ---
    #[error("DNS resolution failed: {0}")]
    DnsFailed(String),
    #[error("STUN binding failed: {0}")]
    StunFailed(String),
    #[error("ICE candidate gathering failed: {0}")]
    IceGatherFailed(String),
    #[error("NAT traversal failed: no usable candidates")]
    IceNoCandidates,
    #[error("TURN relay error: {0}")]
    TurnError(String),

    // --- Plugin errors ---
    #[error("Plugin error: {plugin} - {message}")]
    PluginError { plugin: String, message: String },
    #[error("WASM runtime error: {0}")]
    WasmError(String),

    // --- IO errors ---
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Config error: {0}")]
    ConfigError(String),
    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    // --- Internal errors ---
    #[error("Internal invariant violated: {0}")]
    InvariantViolation(String),
    #[error("State error: expected {expected}, got {got}")]
    StateError { expected: String, got: String },
    #[error("Threading error: {0}")]
    ThreadingError(String),
    #[error("Buffer overflow: requested {requested}, available {available}")]
    BufferOverflow { requested: usize, available: usize },
}

impl ContinuumError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ConnectionFailed(_)
                | Self::ConnectionTimeout(_)
                | Self::RateLimited { .. }
                | Self::StunFailed(_)
                | Self::IceGatherFailed(_)
        )
    }

    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::InvariantViolation(_)
                | Self::BufferOverflow { .. }
                | Self::ProtocolViolation(_)
                | Self::CertVerificationFailed(_)
        )
    }

    pub fn severity(&self) -> &'static str {
        if self.is_fatal() {
            "FATAL"
        } else if self.is_retryable() {
            "RECOVERABLE"
        } else {
            "ERROR"
        }
    }
}

impl From<serde_json::Error> for ContinuumError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

impl From<anyhow::Error> for ContinuumError {
    fn from(e: anyhow::Error) -> Self {
        Self::InvariantViolation(e.to_string())
    }
}

pub type ContinuumResult<T> = Result<T, ContinuumError>;

pub fn ensure(condition: bool, msg: &str) -> ContinuumResult<()> {
    if !condition {
        return Err(ContinuumError::InvariantViolation(msg.to_string()));
    }
    Ok(())
}

pub fn ensure_bounds(value: usize, max: usize, _name: &str) -> ContinuumResult<usize> {
    if value > max {
        return Err(ContinuumError::BufferOverflow {
            requested: value,
            available: max,
        });
    }
    Ok(value)
}

pub fn ensure_length<T>(slice: &[T], expected: usize, name: &str) -> ContinuumResult<()> {
    if slice.len() != expected {
        return Err(ContinuumError::InvariantViolation(format!(
            "{} length mismatch: expected {}, got {}",
            name,
            expected,
            slice.len()
        )));
    }
    Ok(())
}

pub fn state_check(expected: &str, got: &str) -> ContinuumResult<()> {
    Err(ContinuumError::StateError {
        expected: expected.to_string(),
        got: got.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_is_retryable() {
        assert!(ContinuumError::ConnectionFailed("test".into()).is_retryable());
        assert!(ContinuumError::ConnectionTimeout("test".into()).is_retryable());
        assert!(ContinuumError::RateLimited { retry_after: 60 }.is_retryable());
        assert!(ContinuumError::StunFailed("test".into()).is_retryable());
        assert!(!ContinuumError::CaptureFailed("test".into()).is_retryable());
    }

    #[test]
    fn test_error_is_fatal() {
        assert!(ContinuumError::InvariantViolation("test".into()).is_fatal());
        assert!(ContinuumError::BufferOverflow {
            requested: 100,
            available: 50
        }
        .is_fatal());
        assert!(ContinuumError::ProtocolViolation("test".into()).is_fatal());
        assert!(ContinuumError::CertVerificationFailed("test".into()).is_fatal());
        assert!(!ContinuumError::ConnectionFailed("test".into()).is_fatal());
    }

    #[test]
    fn test_error_severity() {
        assert_eq!(
            ContinuumError::InvariantViolation("t".into()).severity(),
            "FATAL"
        );
        assert_eq!(
            ContinuumError::ConnectionFailed("t".into()).severity(),
            "RECOVERABLE"
        );
        assert_eq!(
            ContinuumError::CaptureFailed("t".into()).severity(),
            "ERROR"
        );
    }

    #[test]
    fn test_error_display() {
        let err = ContinuumError::ConnectionFailed("timeout".to_string());
        assert_eq!(format!("{}", err), "Connection failed: timeout");

        let err = ContinuumError::EncodeFailed {
            backend: "jpeg".into(),
            reason: "bad data".into(),
        };
        assert_eq!(format!("{}", err), "Encode failed: jpeg - bad data");
    }

    #[test]
    fn test_ensure_passes() {
        let result = ensure(true, "should pass");
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_fails() {
        let result = ensure(false, "should fail");
        assert!(result.is_err());
    }

    #[test]
    fn test_ensure_bounds_within() {
        let result = ensure_bounds(50, 100, "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 50);
    }

    #[test]
    fn test_ensure_bounds_exceeded() {
        let result = ensure_bounds(150, 100, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_ensure_length_match() {
        let data = vec![1, 2, 3];
        let result = ensure_length(&data, 3, "test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_length_mismatch() {
        let data = vec![1, 2, 3];
        let result = ensure_length(&data, 5, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_state_check_returns_error() {
        let result = state_check("connected", "disconnected");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let continuum_err: ContinuumError = json_err.into();
        match continuum_err {
            ContinuumError::Serialization(_) => {}
            _ => panic!("Expected Serialization error"),
        }
    }

    #[test]
    fn test_from_anyhow_error() {
        let anyhow_err = anyhow::anyhow!("test error");
        let continuum_err: ContinuumError = anyhow_err.into();
        match continuum_err {
            ContinuumError::InvariantViolation(_) => {}
            _ => panic!("Expected InvariantViolation error"),
        }
    }

    #[test]
    fn test_connection_closed_display() {
        let err = ContinuumError::ConnectionClosed("peer reset".into());
        assert!(format!("{}", err).contains("peer reset"));
    }

    #[test]
    fn test_alpn_mismatch_display() {
        let err = ContinuumError::AlpnMismatch {
            expected: "apq-2".into(),
            got: "h2".into(),
        };
        assert!(format!("{}", err).contains("apq-2"));
        assert!(format!("{}", err).contains("h2"));
    }
}
