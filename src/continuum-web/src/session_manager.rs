use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Web session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSession {
    pub id: String,
    pub pairing_code: String,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

/// Web session store
#[derive(Debug, Clone, Default)]
pub struct WebSessionStore {
    sessions: Vec<WebSession>,
}

impl WebSessionStore {
    pub fn new() -> Self {
        Self { sessions: Vec::new() }
    }

    /// Add a session
    pub fn add(&mut self, session: WebSession) {
        self.sessions.push(session);
    }

    /// Find by pairing code
    pub fn find_by_code(&self, code: &str) -> Option<&WebSession> {
        self.sessions.iter().find(|s| s.pairing_code == code)
    }

    /// Get active sessions
    pub fn active(&self) -> Vec<&WebSession> {
        self.sessions.iter().filter(|s| s.is_active).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_find() {
        let mut store = WebSessionStore::new();
        store.add(WebSession {
            id: "test".to_string(),
            pairing_code: "code123".to_string(),
            created_at: Utc::now(),
            is_active: true,
        });
        assert!(store.find_by_code("code123").is_some());
    }
}
