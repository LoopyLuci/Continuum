#![allow(dead_code)]

use anyhow::Result;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct IceConfig {
    pub stun_servers: Vec<String>,
    pub turn_servers: Vec<TurnServerConfig>,
    pub ice_lite: bool,
    pub nomination_mode: NominationMode,
    pub gather_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct TurnServerConfig {
    pub url: String,
    pub username: String,
    pub credential: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NominationMode {
    Regular,
    Aggressive,
}

impl Default for IceConfig {
    fn default() -> Self {
        Self {
            stun_servers: vec!["stun.l.google.com:19302".to_string()],
            turn_servers: Vec::new(),
            ice_lite: true,
            nomination_mode: NominationMode::Aggressive,
            gather_timeout_secs: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IceCandidate {
    pub addr: SocketAddr,
    pub kind: CandidateKind,
    pub priority: u32,
    pub foundation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Host,
    Srflx,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IceState {
    Gathering,
    Connecting,
    Connected,
    Failed,
    Closed,
}

pub struct IceTransport {
    config: IceConfig,
    state: IceState,
    local_candidates: Vec<IceCandidate>,
    remote_candidates: Vec<IceCandidate>,
    selected_pair: Option<(IceCandidate, IceCandidate)>,
    socket: Option<UdpSocket>,
    tiebreaker: u64,
}

impl IceTransport {
    pub fn new(config: IceConfig) -> Self {
        Self {
            config,
            state: IceState::Gathering,
            local_candidates: Vec::new(),
            remote_candidates: Vec::new(),
            selected_pair: None,
            socket: None,
            tiebreaker: rand::random(),
        }
    }

    pub async fn gather_candidates(&mut self) -> Result<Vec<IceCandidate>> {
        let local_addrs = resolve_local_addrs().await;
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let _local_addr = socket.local_addr()?;

        let mut candidates = Vec::new();

        for addr in &local_addrs {
            candidates.push(IceCandidate {
                addr: *addr,
                kind: CandidateKind::Host,
                priority: calc_priority(CandidateKind::Host, candidates.len() as u32),
                foundation: "host".to_string(),
            });
        }

        for stun_addr in &self.config.stun_servers {
            if let Ok(stun_addr) = stun_addr.parse::<SocketAddr>() {
                if let Ok(srflx) = stun_bind(&socket, stun_addr).await {
                    candidates.push(IceCandidate {
                        addr: srflx,
                        kind: CandidateKind::Srflx,
                        priority: calc_priority(CandidateKind::Srflx, candidates.len() as u32),
                        foundation: format!("srflx-{}", stun_addr.port()),
                    });
                }
            }
        }

        candidates.sort_by_key(|b| std::cmp::Reverse(b.priority));
        self.local_candidates = candidates.clone();
        self.socket = Some(socket);

        if !self.config.ice_lite {
            self.state = IceState::Connecting;
        } else {
            self.state = IceState::Connected;
        }

        tracing::info!(count = candidates.len(), "ICE candidates gathered");
        Ok(candidates)
    }

    pub fn set_remote_candidates(&mut self, candidates: Vec<IceCandidate>) {
        self.remote_candidates = candidates;
        if let (Some(local), Some(remote)) = (
            self.local_candidates.first(),
            self.remote_candidates.first(),
        ) {
            self.selected_pair = Some((local.clone(), remote.clone()));
            tracing::info!(local = %local.addr, remote = %remote.addr, "ICE pair selected");
        }
    }

    pub fn selected_connection(&self) -> Option<SocketAddr> {
        self.selected_pair.as_ref().map(|(_, r)| r.addr)
    }

    pub fn local_candidates(&self) -> &[IceCandidate] {
        &self.local_candidates
    }

    pub fn remote_candidates(&self) -> &[IceCandidate] {
        &self.remote_candidates
    }

    pub fn state(&self) -> &IceState {
        &self.state
    }
}

async fn stun_bind(socket: &UdpSocket, stun_addr: SocketAddr) -> Result<SocketAddr> {
    let mut req = vec![0u8; 20];
    req[0] = 0x00;
    req[1] = 0x01;
    let msg_len: u16 = 0;
    req[2..4].copy_from_slice(&msg_len.to_be_bytes());
    let cookie: u32 = 0x2112A442;
    req[4..8].copy_from_slice(&cookie.to_be_bytes());
    let tx_id: u128 = rand::random();
    req[8..20].copy_from_slice(&tx_id.to_be_bytes());

    socket.send_to(&req, stun_addr).await?;

    let mut buf = vec![0u8; 512];
    let result = timeout(Duration::from_secs(3), socket.recv_from(&mut buf)).await;
    match result {
        Ok(Ok((len, _))) => {
            let mapped_addr = parse_stun_response(&buf[..len], tx_id);
            match mapped_addr {
                Some(addr) => Ok(addr),
                None => Err(anyhow::anyhow!("STUN: no mapped address")),
            }
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("STUN recv error: {}", e)),
        Err(_) => Err(anyhow::anyhow!("STUN timeout")),
    }
}

fn parse_stun_response(data: &[u8], _tx_id: u128) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }
    if data[0] != 0x01 || data[1] != 0x01 {
        return None;
    }
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let mut offset = 20usize;
    let end = 20 + msg_len;

    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > data.len() {
            break;
        }

        if attr_type == 0x0020 && attr_len >= 8 {
            let port = u16::from_be_bytes([data[offset + 2], data[offset + 3]]);
            let ip = std::net::Ipv4Addr::new(
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            );
            return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
        }
        offset += (attr_len + 3) & !3;
    }
    None
}

fn calc_priority(kind: CandidateKind, index: u32) -> u32 {
    let type_pref = match kind {
        CandidateKind::Host => 126u32,
        CandidateKind::Srflx => 100,
        CandidateKind::Relay => 50,
    };
    (type_pref << 24) | (1 << 8) | (256 - index)
}

async fn resolve_local_addrs() -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    if let Ok(hostname) = hostname::get() {
        if let Ok(ips) = tokio::net::lookup_host(format!("{}:0", hostname.to_string_lossy())).await
        {
            for ip in ips {
                if ip.is_ipv4() && !ip.ip().is_loopback() {
                    addrs.push(SocketAddr::new(ip.ip(), 0));
                }
            }
        }
    }
    if addrs.is_empty() {
        addrs.push("0.0.0.0:0".parse().expect("Hardcoded address is valid"));
    }
    addrs
}

#[async_trait::async_trait]
pub trait IceConnect: Send + Sync {
    async fn connect(&mut self, addr: SocketAddr) -> Result<IceState>;
    fn local_addr(&self) -> Option<SocketAddr>;
    fn remote_addr(&self) -> Option<SocketAddr>;
}
