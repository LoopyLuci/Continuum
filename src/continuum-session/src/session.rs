use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Peer permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permissions {
    pub can_view: bool,
    pub can_control: bool,
    pub can_transfer_files: bool,
    pub can_clipboard: bool,
    pub can_audio: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            can_view: true,
            can_control: true,
            can_transfer_files: true,
            can_clipboard: true,
            can_audio: true,
        }
    }
}

/// A peer in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPeer {
    pub machine_id: String,
    pub display_name: String,
    pub joined_at: DateTime<Utc>,
    pub permissions: Permissions,
}

/// Session state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    WaitingForPeer,
    Active,
    Paused,
    Closed,
}

/// Session capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCapabilities {
    pub max_viewers: usize,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    pub allow_file_transfer: bool,
    pub allow_audio: bool,
}

impl Default for SessionCapabilities {
    fn default() -> Self {
        Self {
            max_viewers: 1,
            allow_input: true,
            allow_clipboard: true,
            allow_file_transfer: true,
            allow_audio: true,
        }
    }
}

/// A session between two or more peers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub host: SessionPeer,
    pub viewers: Vec<SessionPeer>,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub pairing_code: String,
    pub capabilities: SessionCapabilities,
}

/// Session store for managing active sessions
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session
    pub async fn create(&self, host: SessionPeer, pairing_code: String) -> String {
        let id = Uuid::new_v4().to_string();
        let session = Session {
            id: id.clone(),
            host,
            viewers: Vec::new(),
            state: SessionState::WaitingForPeer,
            created_at: Utc::now(),
            pairing_code,
            capabilities: SessionCapabilities::default(),
        };
        self.sessions.write().await.insert(id.clone(), session);
        id
    }

    /// Join an existing session
    pub async fn join(&self, session_id: &str, viewer: SessionPeer) -> continuum_core::ContinuumResult<()> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id)
            .ok_or_else(|| continuum_core::ContinuumError::Internal("Session not found".into()))?;
        
        if session.viewers.len() >= session.capabilities.max_viewers {
            return Err(continuum_core::ContinuumError::NotSupported("Session is full".into()));
        }
        
        session.viewers.push(viewer);
        if session.state == SessionState::WaitingForPeer {
            session.state = SessionState::Active;
        }
        Ok(())
    }

    /// Get session by pairing code
    pub async fn find_by_code(&self, code: &str) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.values().find(|s| s.pairing_code == code).cloned()
    }

    /// Get session by ID
    pub async fn get(&self, session_id: &str) -> Option<Session> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// Close a session
    pub async fn close(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
    }

    /// List all active sessions
    pub async fn list_active(&self) -> Vec<Session> {
        self.sessions.read().await
            .values()
            .filter(|s| s.state == SessionState::Active)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_peer(name: &str) -> SessionPeer {
        SessionPeer {
            machine_id: name.to_string(),
            display_name: name.to_string(),
            joined_at: Utc::now(),
            permissions: Permissions::default(),
        }
    }

    #[tokio::test]
    async fn test_create_session() {
        let store = SessionStore::new();
        let host = create_test_peer("host");
        let id = store.create(host, "code123".to_string()).await;
        
        let session = store.get(&id).await.unwrap();
        assert_eq!(session.pairing_code, "code123");
        assert_eq!(session.state, SessionState::WaitingForPeer);
    }

    #[tokio::test]
    async fn test_join_session() {
        let store = SessionStore::new();
        let host = create_test_peer("host");
        let id = store.create(host, "code123".to_string()).await;
        
        let viewer = create_test_peer("viewer");
        store.join(&id, viewer).await.unwrap();
        
        let session = store.get(&id).await.unwrap();
        assert_eq!(session.viewers.len(), 1);
        assert_eq!(session.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_find_by_code() {
        let store = SessionStore::new();
        let host = create_test_peer("host");
        store.create(host, "code123".to_string()).await;
        
        let session = store.find_by_code("code123").await;
        assert!(session.is_some());
        assert_eq!(session.unwrap().pairing_code, "code123");
    }

    #[tokio::test]
    async fn test_close_session() {
        let store = SessionStore::new();
        let host = create_test_peer("host");
        let id = store.create(host, "code123".to_string()).await;
        
        store.close(&id).await;
        assert!(store.get(&id).await.is_none());
    }
}
