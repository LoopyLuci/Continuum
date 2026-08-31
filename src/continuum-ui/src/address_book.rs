use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Online,
    Offline,
    Away,
    Busy,
}

/// Address book entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBookEntry {
    pub machine_id: String,
    pub display_name: String,
    pub last_connected: DateTime<Utc>,
    pub last_address: String,
    pub status: ConnectionStatus,
    pub notes: String,
}

/// Address book for saved connections
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddressBook {
    entries: Vec<AddressBookEntry>,
}

impl AddressBook {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Add a new entry
    pub fn add(&mut self, entry: AddressBookEntry) {
        self.entries.push(entry);
    }

    /// Remove an entry by machine ID
    pub fn remove(&mut self, machine_id: &str) -> bool {
        let len = self.entries.len();
        self.entries.retain(|e| e.machine_id != machine_id);
        self.entries.len() < len
    }

    /// Get an entry by machine ID
    pub fn get(&self, machine_id: &str) -> Option<&AddressBookEntry> {
        self.entries.iter().find(|e| e.machine_id == machine_id)
    }

    /// Get all entries
    pub fn entries(&self) -> &[AddressBookEntry] {
        &self.entries
    }

    /// Get entries sorted by last connected (most recent first)
    pub fn recent(&self) -> Vec<&AddressBookEntry> {
        let mut sorted: Vec<_> = self.entries.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.last_connected));
        sorted
    }

    /// Get online entries
    pub fn online(&self) -> Vec<&AddressBookEntry> {
        self.entries.iter().filter(|e| e.status == ConnectionStatus::Online).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_entry() {
        let mut book = AddressBook::new();
        let entry = AddressBookEntry {
            machine_id: "test-1".to_string(),
            display_name: "Test Machine".to_string(),
            last_connected: Utc::now(),
            last_address: "192.168.1.1:4433".to_string(),
            status: ConnectionStatus::Online,
            notes: String::new(),
        };
        book.add(entry);
        assert!(book.get("test-1").is_some());
    }

    #[test]
    fn test_remove_entry() {
        let mut book = AddressBook::new();
        book.add(AddressBookEntry {
            machine_id: "test-1".to_string(),
            display_name: "Test".to_string(),
            last_connected: Utc::now(),
            last_address: "192.168.1.1:4433".to_string(),
            status: ConnectionStatus::Online,
            notes: String::new(),
        });
        assert!(book.remove("test-1"));
        assert!(book.get("test-1").is_none());
    }

    #[test]
    fn test_online_filter() {
        let mut book = AddressBook::new();
        book.add(AddressBookEntry {
            machine_id: "online-1".to_string(),
            display_name: "Online".to_string(),
            last_connected: Utc::now(),
            last_address: "192.168.1.1:4433".to_string(),
            status: ConnectionStatus::Online,
            notes: String::new(),
        });
        book.add(AddressBookEntry {
            machine_id: "offline-1".to_string(),
            display_name: "Offline".to_string(),
            last_connected: Utc::now(),
            last_address: "192.168.1.2:4433".to_string(),
            status: ConnectionStatus::Offline,
            notes: String::new(),
        });
        assert_eq!(book.online().len(), 1);
    }
}
