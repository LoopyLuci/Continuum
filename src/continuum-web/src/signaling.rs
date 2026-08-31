use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// Signaling message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalingMessage {
    JoinSession {
        pairing_code: String,
        peer_id: String,
    },
    LeaveSession {
        session_id: String,
        peer_id: String,
    },
    IceCandidate {
        session_id: String,
        peer_id: String,
        candidate: String,
    },
    SdpOffer {
        session_id: String,
        peer_id: String,
        sdp: String,
    },
    SdpAnswer {
        session_id: String,
        peer_id: String,
        sdp: String,
    },
    SessionJoined {
        session_id: String,
        peer_id: String,
    },
    SessionLeft {
        session_id: String,
        peer_id: String,
    },
    Error {
        message: String,
    },
}

/// Peer connection info
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub id: String,
    pub display_name: String,
    pub joined_at: DateTime<Utc>,
    pub sender: mpsc::Sender<SignalingMessage>,
}

/// Session info
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub pairing_code: String,
    pub peers: HashMap<String, PeerInfo>,
    pub created_at: DateTime<Utc>,
}

/// Session manager for web signaling
#[derive(Debug, Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, SessionInfo>>>,
    by_pairing_code: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            by_pairing_code: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session
    pub async fn create_session(&self, pairing_code: String) -> String {
        let id = Uuid::new_v4().to_string();
        let session = SessionInfo {
            id: id.clone(),
            pairing_code: pairing_code.clone(),
            peers: HashMap::new(),
            created_at: Utc::now(),
        };
        self.sessions.write().await.insert(id.clone(), session);
        self.by_pairing_code.write().await.insert(pairing_code, id.clone());
        id
    }

    /// Join a session by pairing code
    pub async fn join_session(
        &self,
        pairing_code: &str,
        peer: PeerInfo,
    ) -> Option<String> {
        let by_code = self.by_pairing_code.read().await;
        let session_id = by_code.get(pairing_code)?.clone();
        drop(by_code);

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.peers.insert(peer.id.clone(), peer);
            Some(session_id)
        } else {
            None
        }
    }

    /// Leave a session
    pub async fn leave_session(&self, session_id: &str, peer_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.peers.remove(peer_id);
            if session.peers.is_empty() {
                let code = session.pairing_code.clone();
                drop(sessions);
                self.sessions.write().await.remove(session_id);
                self.by_pairing_code.write().await.remove(&code);
            }
        }
    }

    /// Send message to a specific peer in a session
    pub async fn send_to_peer(
        &self,
        session_id: &str,
        peer_id: &str,
        message: SignalingMessage,
    ) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            if let Some(peer) = session.peers.get(peer_id) {
                return peer.sender.send(message).await.is_ok();
            }
        }
        false
    }

    /// Broadcast message to all peers in a session except sender
    pub async fn broadcast(
        &self,
        session_id: &str,
        exclude_peer_id: &str,
        message: SignalingMessage,
    ) {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            for (id, peer) in &session.peers {
                if id != exclude_peer_id {
                    let _ = peer.sender.send(message.clone()).await;
                }
            }
        }
    }

    /// Get session info
    pub async fn get_session(&self, session_id: &str) -> Option<SessionInfo> {
        self.sessions.read().await.get(session_id).cloned()
    }
}

/// Signaling server (placeholder for WebSocket integration)
pub struct SignalingServer {
    session_manager: SessionManager,
}

impl SignalingServer {
    pub fn new() -> Self {
        Self {
            session_manager: SessionManager::new(),
        }
    }

    /// Get session manager
    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_peer(id: &str) -> PeerInfo {
        let (tx, _rx) = mpsc::channel(100);
        PeerInfo {
            id: id.to_string(),
            display_name: id.to_string(),
            joined_at: Utc::now(),
            sender: tx,
        }
    }

    #[tokio::test]
    async fn test_create_session() {
        let manager = SessionManager::new();
        let id = manager.create_session("code123".to_string()).await;
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn test_join_session() {
        let manager = SessionManager::new();
        let session_id = manager.create_session("code123".to_string()).await;
        let peer = create_test_peer("peer-1");
        let result = manager.join_session("code123", peer).await;
        assert_eq!(result, Some(session_id));
    }

    #[tokio::test]
    async fn test_leave_session() {
        let manager = SessionManager::new();
        let session_id = manager.create_session("code123".to_string()).await;
        let peer = create_test_peer("peer-1");
        manager.join_session("code123", peer).await;
        manager.leave_session(&session_id, "peer-1").await;
        assert!(manager.get_session(&session_id).await.is_none());
    }
}
