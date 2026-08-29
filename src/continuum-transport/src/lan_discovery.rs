#![allow(dead_code)]

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SERVICE_TYPE: &str = "_continuum._udp.local";
const DISCOVERY_PORT: u16 = 5353;
const BROADCAST_INTERVAL: Duration = Duration::from_secs(30);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub name: String,
    pub addr: SocketAddr,
    pub hostname: String,
    pub version: String,
    pub pairing_code_required: bool,
    pub supports_clipboard: bool,
    pub supports_audio: bool,
    pub first_seen: Instant,
    pub last_seen: Instant,
}

pub struct LanDiscovery {
    servers: Arc<Mutex<HashMap<String, DiscoveredServer>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl LanDiscovery {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        self.running
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let servers = self.servers.clone();
        let running = self.running.clone();

        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.set_broadcast(true)?;

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while running.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(Ok((len, addr))) =
                    tokio::time::timeout(DISCOVERY_TIMEOUT, socket.recv_from(&mut buf)).await
                {
                    if let Some(server) = Self::parse_response(&buf[..len], addr) {
                        let mut map = servers.lock().await;
                        let entry =
                            map.entry(server.name.clone())
                                .or_insert_with(|| DiscoveredServer {
                                    first_seen: Instant::now(),
                                    ..server.clone()
                                });
                        entry.last_seen = Instant::now();
                        entry.addr = server.addr;
                    }
                }
            }
        });

        tracing::info!("LAN discovery started");
        Ok(())
    }

    pub async fn broadcast_presence(
        &self,
        listen_addr: SocketAddr,
        server_name: &str,
    ) -> Result<()> {
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.set_broadcast(true)?;

        let payload = Self::build_announcement(listen_addr, server_name);
        let broadcast_addr: SocketAddr = "255.255.255.255:5353"
            .parse()
            .expect("Hardcoded broadcast address is valid");

        tokio::spawn(async move {
            loop {
                let _ = socket.send_to(&payload, broadcast_addr).await;
                tokio::time::sleep(BROADCAST_INTERVAL).await;
            }
        });

        Ok(())
    }

    pub async fn servers(&self) -> Vec<DiscoveredServer> {
        let mut map = self.servers.lock().await;
        map.retain(|_, s| s.last_seen.elapsed() < Duration::from_secs(120));
        let mut list: Vec<DiscoveredServer> = map.values().cloned().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.last_seen));
        list
    }

    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn build_announcement(listen_addr: SocketAddr, server_name: &str) -> Vec<u8> {
        let info = serde_json::json!({
            "type": "continuum-announce",
            "version": env!("CARGO_PKG_VERSION"),
            "name": server_name,
            "port": listen_addr.port(),
            "hostname": hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_default(),
        });
        serde_json::to_vec(&info).unwrap_or_default()
    }

    fn parse_response(data: &[u8], addr: SocketAddr) -> Option<DiscoveredServer> {
        let info: serde_json::Value = serde_json::from_slice(data).ok()?;
        if info.get("type")?.as_str()? != "continuum-announce" {
            return None;
        }
        let name = info.get("name")?.as_str()?.to_string();
        let port = info.get("port")?.as_u64()? as u16;
        let version = info.get("version")?.as_str().unwrap_or("0.0.0").to_string();
        let hostname = info.get("hostname")?.as_str().unwrap_or("").to_string();

        Some(DiscoveredServer {
            name,
            addr: SocketAddr::new(addr.ip(), port),
            hostname,
            version,
            pairing_code_required: true,
            supports_clipboard: true,
            supports_audio: true,
            first_seen: Instant::now(),
            last_seen: Instant::now(),
        })
    }
}

impl Default for LanDiscovery {
    fn default() -> Self {
        Self::new()
    }
}
