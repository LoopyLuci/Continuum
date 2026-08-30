use std::net::SocketAddr;

/// ICE candidate types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IceCandidateType {
    Host,
    ServerReflexive,
    PeerReflexive,
    Relayed,
}

impl IceCandidateType {
    pub fn priority(&self) -> u32 {
        match self {
            Self::Host => 2130706431,
            Self::ServerReflexive => 1694498815,
            Self::PeerReflexive => 1694498815,
            Self::Relayed => 16777215,
        }
    }
}

/// An ICE candidate
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCandidate {
    pub candidate_type: IceCandidateType,
    pub address: SocketAddr,
    pub priority: u32,
    pub foundation: String,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Udp,
    Tcp,
}

impl IceCandidate {
    /// Create a host candidate from a local address
    pub fn host(address: SocketAddr) -> Self {
        Self {
            candidate_type: IceCandidateType::Host,
            address,
            priority: IceCandidateType::Host.priority(),
            foundation: format!("host-{}", address),
            protocol: Protocol::Udp,
        }
    }

    /// Create a server reflexive candidate from STUN
    pub fn server_reflexive(address: SocketAddr) -> Self {
        Self {
            candidate_type: IceCandidateType::ServerReflexive,
            address,
            priority: IceCandidateType::ServerReflexive.priority(),
            foundation: format!("srflx-{}", address),
            protocol: Protocol::Udp,
        }
    }

    /// Create a relayed candidate from TURN
    pub fn relayed(address: SocketAddr) -> Self {
        Self {
            candidate_type: IceCandidateType::Relayed,
            address,
            priority: IceCandidateType::Relayed.priority(),
            foundation: format!("relay-{}", address),
            protocol: Protocol::Udp,
        }
    }

    /// Get the SDP-style candidate string
    pub fn to_sdp(&self) -> String {
        let typ = match self.candidate_type {
            IceCandidateType::Host => "host",
            IceCandidateType::ServerReflexive => "srflx",
            IceCandidateType::PeerReflexive => "prflx",
            IceCandidateType::Relayed => "relay",
        };
        let proto = match self.protocol {
            Protocol::Udp => "UDP",
            Protocol::Tcp => "TCP",
        };
        format!(
            "{} {} {} {} {} {} typ {}",
            self.foundation,
            1,
            proto,
            self.priority,
            self.address.ip(),
            self.address.port(),
            typ
        )
    }
}

/// A candidate pair for connectivity checking
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCandidatePair {
    pub local: IceCandidate,
    pub remote: IceCandidate,
    pub priority: u64,
    pub state: CandidatePairState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePairState {
    Waiting,
    InProgress,
    Succeeded,
    Failed,
    Frozen,
}

impl IceCandidatePair {
    pub fn new(local: IceCandidate, remote: IceCandidate) -> Self {
        let priority = Self::calculate_priority(&local, &remote);
        Self {
            local,
            remote,
            priority,
            state: CandidatePairState::Waiting,
        }
    }

    /// Calculate pair priority (RFC 5245)
    fn calculate_priority(local: &IceCandidate, remote: &IceCandidate) -> u64 {
        let g = local.priority as u64;
        let d = remote.priority as u64;
        if g > d {
            (g << 32) + (d * 2) + 1
        } else {
            (d << 32) + (g * 2)
        }
    }
}
