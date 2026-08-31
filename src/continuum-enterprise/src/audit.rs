use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Audit event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditEventType {
    ConnectionRequested,
    ConnectionAccepted,
    ConnectionRejected,
    ConnectionClosed,
    PairingCompleted,
    PairingFailed,
    FileTransfer,
    InputEvent,
    SettingsChanged,
    UserAuthenticated,
    UserAccessDenied,
}

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub actor: String,
    pub target: String,
    pub details: serde_json::Value,
    pub hash: String,
    pub previous_hash: String,
}

/// Audit sink trait
pub trait AuditSink: Send + Sync {
    /// Log an audit event
    fn log(&self, event: &AuditEvent) -> Result<(), AuditError>;
}

/// Audit errors
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// File-based audit sink
pub struct FileAuditSink {
    path: String,
}

impl FileAuditSink {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

impl AuditSink for FileAuditSink {
    fn log(&self, event: &AuditEvent) -> Result<(), AuditError> {
        let json = serde_json::to_string(event)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        std::io::Write::write_all(&mut file, json.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\n")?;
        Ok(())
    }
}

/// Audit logger
pub struct AuditLogger {
    sink: Box<dyn AuditSink>,
    previous_hash: String,
}

impl AuditLogger {
    pub fn new(sink: Box<dyn AuditSink>) -> Self {
        Self {
            sink,
            previous_hash: String::new(),
        }
    }

    /// Log an event
    pub fn log(
        &mut self,
        event_type: AuditEventType,
        actor: &str,
        target: &str,
        details: serde_json::Value,
    ) -> Result<(), AuditError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.previous_hash.as_bytes());
        hasher.update(serde_json::to_string(&details)?.as_bytes());
        let hash = hasher.finalize().to_string();

        let event = AuditEvent {
            timestamp: Utc::now(),
            event_type,
            actor: actor.to_string(),
            target: target.to_string(),
            details,
            hash: hash.clone(),
            previous_hash: self.previous_hash.clone(),
        };

        self.sink.log(&event)?;
        self.previous_hash = hash;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_audit_sink() {
        let sink = FileAuditSink::new("/tmp/test_audit.log".to_string());
        let event = AuditEvent {
            timestamp: Utc::now(),
            event_type: AuditEventType::ConnectionAccepted,
            actor: "test".to_string(),
            target: "peer".to_string(),
            details: serde_json::json!({}),
            hash: "hash".to_string(),
            previous_hash: "prev".to_string(),
        };
        assert!(sink.log(&event).is_ok());
    }
}
