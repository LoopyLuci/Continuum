use crate::candidate::{IceCandidate, IceCandidatePair, IceCandidateType, CandidatePairState};
use crate::stun::{StunMessage, StunClass};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

/// ICE agent configuration
#[derive(Debug, Clone)]
pub struct IceConfig {
    pub stun_servers: Vec<String>,
    pub turn_servers: Vec<String>,
    pub turn_username: String,
    pub turn_password: String,
    pub keep_alive_interval: Duration,
    pub connectivity_check_timeout: Duration,
    pub aggressive_nomination: bool,
}

impl Default for IceConfig {
    fn default() -> Self {
        Self {
            stun_servers: vec![
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun2.l.google.com:19302".to_string(),
                "stun:stun.continuum.local:3478".to_string(),
            ],
            turn_servers: Vec::new(),
            turn_username: String::new(),
            turn_password: String::new(),
            keep_alive_interval: Duration::from_secs(15),
            connectivity_check_timeout: Duration::from_secs(5),
            aggressive_nomination: true,
        }
    }
}

/// ICE agent state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceState {
    New,
    Gathering,
    Waiting,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

/// ICE agent for NAT traversal
pub struct IceAgent {
    config: IceConfig,
    state: IceState,
    local_candidates: Vec<IceCandidate>,
    remote_candidates: Vec<IceCandidate>,
    candidate_pairs: Vec<IceCandidatePair>,
    nominated_pair: Option<IceCandidatePair>,
    udp_socket: Option<UdpSocket>,
}

impl IceAgent {
    /// Create a new ICE agent
    pub fn new(config: IceConfig) -> Self {
        Self {
            config,
            state: IceState::New,
            local_candidates: Vec::new(),
            remote_candidates: Vec::new(),
            candidate_pairs: Vec::new(),
            nominated_pair: None,
            udp_socket: None,
        }
    }

    /// Gather local candidates (host + server reflexive)
    pub async fn gather_candidates(&mut self) -> continuum_core::ContinuumResult<Vec<IceCandidate>> {
        self.state = IceState::Gathering;
        
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let local_addr = socket.local_addr()?;
        self.udp_socket = Some(socket);

        // Host candidate
        self.local_candidates.push(IceCandidate::host(local_addr));

        // Discover public address via STUN
        let stun_candidates = self.discover_stun_candidates().await?;
        self.local_candidates.extend(stun_candidates);

        self.state = IceState::Waiting;
        Ok(self.local_candidates.clone())
    }

    /// Discover server reflexive candidates via STUN
    async fn discover_stun_candidates(&self) -> continuum_core::ContinuumResult<Vec<IceCandidate>> {
        let mut candidates = Vec::new();
        let socket = match &self.udp_socket {
            Some(s) => s,
            None => return Ok(candidates),
        };

        for stun_server in &self.config.stun_servers {
            if let Ok(addr) = self.query_stun_server(socket, stun_server).await {
                candidates.push(IceCandidate::server_reflexive(addr));
            }
        }

        Ok(candidates)
    }

    /// Query a single STUN server
    async fn query_stun_server(
        &self,
        socket: &UdpSocket,
        server: &str,
    ) -> continuum_core::ContinuumResult<SocketAddr> {
        let addr: SocketAddr = server.parse()
            .map_err(|e| continuum_core::ContinuumError::InvalidAddress(format!("Invalid STUN server: {}", e)))?;

        let request = StunMessage::binding_request();
        let bytes = request.to_bytes();

        socket.send_to(&bytes, addr).await?;
        
        let mut buf = [0u8; 1500];
        let (len, _) = tokio::time::timeout(
            self.config.connectivity_check_timeout,
            socket.recv_from(&mut buf),
        ).await.map_err(|_| continuum_core::ContinuumError::Timeout)??;

        let response = StunMessage::from_bytes(&buf[..len])
            .map_err(|e| continuum_core::ContinuumError::Stun(e.to_string()))?;

        match response.class {
            StunClass::ResponseSuccess => {
                response.get_mapped_address()
                    .ok_or(continuum_core::ContinuumError::Stun("No mapped address".into()))
            }
            StunClass::ResponseError => {
                Err(continuum_core::ContinuumError::Stun("STUN error response".into()))
            }
            _ => Err(continuum_core::ContinuumError::Stun("Unexpected response".into())),
        }
    }

    /// Add remote candidates received via signaling
    pub fn add_remote_candidates(&mut self, candidates: Vec<IceCandidate>) {
        self.remote_candidates.extend(candidates);
        self.form_candidate_pairs();
    }

    /// Form candidate pairs from local and remote candidates
    fn form_candidate_pairs(&mut self) {
        self.candidate_pairs.clear();
        for local in &self.local_candidates {
            for remote in &self.remote_candidates {
                if local.protocol == remote.protocol {
                    self.candidate_pairs.push(IceCandidatePair::new(
                        local.clone(),
                        remote.clone(),
                    ));
                }
            }
        }
        self.candidate_pairs.sort_by_key(|p| std::cmp::Reverse(p.priority));
    }

    /// Perform connectivity checks
    pub async fn perform_connectivity_checks(&mut self) -> continuum_core::ContinuumResult<Option<IceCandidatePair>> {
        self.state = IceState::Connecting;

        for pair in &mut self.candidate_pairs {
            if pair.state != CandidatePairState::Waiting {
                continue;
            }

            pair.state = CandidatePairState::InProgress;
            
            let socket = self.udp_socket.as_ref()
                .ok_or(continuum_core::ContinuumError::Internal("No socket".into()))?;

            let request = StunMessage::binding_request();
            let bytes = request.to_bytes();

            if socket.send_to(&bytes, pair.remote.address).await.is_ok() {
                let mut buf = [0u8; 1500];
                match tokio::time::timeout(
                    self.config.connectivity_check_timeout,
                    socket.recv_from(&mut buf),
                ).await {
                    Ok(Ok((len, _))) => {
                        if let Ok(response) = StunMessage::from_bytes(&buf[..len]) {
                            if response.class == StunClass::ResponseSuccess {
                                pair.state = CandidatePairState::Succeeded;
                                self.nominated_pair = Some(pair.clone());
                                self.state = IceState::Connected;
                                return Ok(Some(pair.clone()));
                            }
                        }
                    }
                    _ => {
                        pair.state = CandidatePairState::Failed;
                    }
                }
            } else {
                pair.state = CandidatePairState::Failed;
            }
        }

        self.state = IceState::Failed;
        Ok(None)
    }

    /// Get the nominated (active) candidate pair
    pub fn get_active_pair(&self) -> Option<&IceCandidatePair> {
        self.nominated_pair.as_ref()
    }

    /// Get current state
    pub fn state(&self) -> IceState {
        self.state
    }

    /// Get local candidates
    pub fn local_candidates(&self) -> &[IceCandidate] {
        &self.local_candidates
    }

    /// Get remote candidates
    pub fn remote_candidates(&self) -> &[IceCandidate] {
        &self.remote_candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_priority() {
        let host = IceCandidate::host("192.168.1.1:5000".parse().unwrap());
        let srflx = IceCandidate::server_reflexive("1.2.3.4:5000".parse().unwrap());
        assert!(host.priority > srflx.priority);
    }

    #[test]
    fn test_candidate_pair_priority() {
        let local = IceCandidate::host("192.168.1.1:5000".parse().unwrap());
        let remote = IceCandidate::host("192.168.1.2:5000".parse().unwrap());
        let pair = IceCandidatePair::new(local, remote);
        assert!(pair.priority > 0);
    }

    #[test]
    fn test_form_candidate_pairs() {
        let mut agent = IceAgent::new(IceConfig::default());
        agent.local_candidates.push(IceCandidate::host("192.168.1.1:5000".parse().unwrap()));
        agent.remote_candidates.push(IceCandidate::host("192.168.1.2:5000".parse().unwrap()));
        agent.form_candidate_pairs();
        assert_eq!(agent.candidate_pairs.len(), 1);
    }
}
