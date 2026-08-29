use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEvent,
    pub client_addr: String,
    pub session_id: String,
    pub details: String,
    pub previous_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEvent {
    ConnectionOpened,
    ConnectionClosed,
    PairingAttempt {
        success: bool,
    },
    InputEvent {
        action: String,
    },
    FileTransfer {
        filename: String,
        size: u64,
    },
    ClipboardAccess,
    SettingsChanged {
        key: String,
    },
    Error {
        code: u32,
    },
    FrameSent {
        frame_number: u64,
        size_bytes: u64,
        quality: u8,
        encode_time_us: u64,
        encrypted: bool,
    },
    SessionEnded,
}

pub struct AuditLogger {
    entries: Mutex<Vec<AuditEntry>>,
    max_entries: usize,
    file_writer: Option<Mutex<std::io::BufWriter<std::fs::File>>>,
}

impl AuditLogger {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            max_entries: 10000,
            file_writer: None,
        }
    }

    pub fn new_with_file(path: &std::path::Path) -> Result<Self, std::io::Error> {
        let file = std::fs::File::create(path)?;
        let writer = std::io::BufWriter::new(file);
        Ok(Self {
            entries: Mutex::new(Vec::new()),
            max_entries: 10000,
            file_writer: Some(Mutex::new(writer)),
        })
    }

    pub fn log(
        &self,
        event_type: AuditEvent,
        client_addr: &str,
        session_id: &str,
        details: &str,
    ) -> AuditEntry {
        let previous_hash = {
            let entries = self.entries.lock().unwrap();
            entries
                .last()
                .map(|e| e.hash.clone())
                .unwrap_or_else(|| "genesis".to_string())
        };

        let entry = AuditEntry {
            timestamp: Utc::now(),
            event_type,
            client_addr: client_addr.to_string(),
            session_id: session_id.to_string(),
            details: details.to_string(),
            previous_hash,
            hash: String::new(),
        };

        let hash = Self::compute_hash(&entry);
        let mut entry = entry;
        entry.hash = hash;

        let mut entries = self.entries.lock().unwrap();
        entries.push(entry.clone());
        if entries.len() > self.max_entries {
            entries.remove(0);
        }

        // Write to file if audit-frames is enabled
        if let Some(ref writer) = self.file_writer {
            if let Ok(line) = serde_json::to_string(&entry) {
                let mut w = writer.lock().unwrap();
                let _ = writeln!(w, "{}", line);
                let _ = w.flush();
            }
        }

        tracing::debug!(
            audit_event = ?entry.event_type,
            client = %entry.client_addr,
            "Audit log"
        );

        entry
    }

    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn export_json(&self) -> String {
        let entries = self.entries.lock().unwrap();
        serde_json::to_string_pretty(&*entries).unwrap_or_default()
    }

    pub fn verify_chain(&self) -> bool {
        let entries = self.entries.lock().unwrap();
        let mut prev_hash = "genesis".to_string();

        for entry in entries.iter() {
            let computed = Self::compute_hash(entry);
            if entry.hash != computed {
                return false;
            }
            if entry.previous_hash != prev_hash {
                return false;
            }
            prev_hash = entry.hash.clone();
        }

        true
    }

    fn compute_hash(entry: &AuditEntry) -> String {
        let mut hasher = Sha256::new();
        hasher.update(entry.timestamp.to_rfc3339().as_bytes());
        hasher.update(format!("{:?}", entry.event_type).as_bytes());
        hasher.update(entry.client_addr.as_bytes());
        hasher.update(entry.session_id.as_bytes());
        hasher.update(entry.details.as_bytes());
        hasher.update(entry.previous_hash.as_bytes());
        hex::encode(hasher.finalize())
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_chain_verification() {
        let log = AuditLogger::new();
        log.log(
            AuditEvent::ConnectionOpened,
            "127.0.0.1:4433",
            "session-1",
            "Connection established",
        );
        log.log(
            AuditEvent::PairingAttempt { success: true },
            "127.0.0.1:4433",
            "session-1",
            "Pairing successful",
        );
        log.log(
            AuditEvent::InputEvent {
                action: "mouse_click".to_string(),
            },
            "127.0.0.1:4433",
            "session-1",
            "Mouse click at (500, 300)",
        );

        assert!(log.verify_chain());
    }

    #[test]
    fn test_audit_tamper_detection() {
        let log = AuditLogger::new();
        log.log(AuditEvent::ConnectionOpened, "127.0.0.1", "s1", "connect");

        {
            let mut entries = log.entries.lock().unwrap();
            if let Some(entry) = entries.first_mut() {
                entry.details = "TAMPERED".to_string();
            }
        }

        assert!(!log.verify_chain());
    }

    #[test]
    fn test_audit_file_writing() {
        let dir = std::env::temp_dir().join("continuum-audit-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("audit.jsonl");

        {
            let log = AuditLogger::new_with_file(&path).expect("create audit file");
            log.log(AuditEvent::ConnectionOpened, "127.0.0.1", "s1", "connect");
            log.log(
                AuditEvent::FrameSent {
                    frame_number: 1,
                    size_bytes: 1024,
                    quality: 85,
                    encode_time_us: 3200,
                    encrypted: true,
                },
                "127.0.0.1",
                "s1",
                "Frame #1",
            );
            log.log(AuditEvent::SessionEnded, "127.0.0.1", "s1", "disconnect");
        }

        let content = std::fs::read_to_string(&path).expect("read audit file");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "Should have 3 audit entries");

        for line in &lines {
            let entry: AuditEntry = serde_json::from_str(line).expect("valid JSON");
            assert!(!entry.hash.is_empty());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
