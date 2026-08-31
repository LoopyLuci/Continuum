use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Connection direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionDirection {
    Inbound,
    Outbound,
}

/// History entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub peer_name: String,
    pub peer_id: String,
    pub duration: Duration,
    pub bytes_transferred: u64,
    pub direction: ConnectionDirection,
}

/// Connection history
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionHistory {
    entries: Vec<HistoryEntry>,
}

impl ConnectionHistory {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Add a new entry
    pub fn add(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
    }

    /// Get all entries
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Get recent entries (last N)
    pub fn recent(&self, n: usize) -> &[HistoryEntry] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }

    /// Get total bytes transferred
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes_transferred).sum()
    }

    /// Get total connection time
    pub fn total_duration(&self) -> Duration {
        self.entries.iter().map(|e| e.duration).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_history() {
        let mut history = ConnectionHistory::new();
        history.add(HistoryEntry {
            timestamp: Utc::now(),
            peer_name: "Test".to_string(),
            peer_id: "test-1".to_string(),
            duration: Duration::from_secs(60),
            bytes_transferred: 1024,
            direction: ConnectionDirection::Outbound,
        });
        assert_eq!(history.entries().len(), 1);
    }

    #[test]
    fn test_total_bytes() {
        let mut history = ConnectionHistory::new();
        history.add(HistoryEntry {
            timestamp: Utc::now(),
            peer_name: "Test".to_string(),
            peer_id: "test-1".to_string(),
            duration: Duration::from_secs(60),
            bytes_transferred: 1024,
            direction: ConnectionDirection::Outbound,
        });
        history.add(HistoryEntry {
            timestamp: Utc::now(),
            peer_name: "Test".to_string(),
            peer_id: "test-2".to_string(),
            duration: Duration::from_secs(60),
            bytes_transferred: 2048,
            direction: ConnectionDirection::Inbound,
        });
        assert_eq!(history.total_bytes(), 3072);
    }
}
