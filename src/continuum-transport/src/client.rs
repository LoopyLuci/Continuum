use crate::config::ClientConfig;
use crate::e2e::E2EEncryptor;
use crate::ice_transport::{IceConfig, IceTransport, NominationMode, TurnServerConfig};
use crate::tls;
use crate::types::*;
use anyhow::Result;
use quinn::{ClientConfig as QuinnClientConfig, Connection, Endpoint, VarInt};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ConnectionMetrics {
    pub latency_ms: f32,
    pub packet_loss_pct: f32,
    pub bandwidth_kbps: f32,
    pub frames_received: u64,
    pub frames_dropped: u64,
    pub bytes_received: u64,
    latency_history: VecDeque<f32>,
    loss_window: VecDeque<bool>,
    bandwidth_window: VecDeque<(Instant, usize)>,
    _last_update: Instant,
}

impl ConnectionMetrics {
    pub fn new() -> Self {
        Self {
            latency_ms: 0.0,
            packet_loss_pct: 0.0,
            bandwidth_kbps: 0.0,
            frames_received: 0,
            frames_dropped: 0,
            bytes_received: 0,
            latency_history: VecDeque::with_capacity(120),
            loss_window: VecDeque::with_capacity(100),
            bandwidth_window: VecDeque::with_capacity(100),
            _last_update: Instant::now(),
        }
    }

    pub fn record_frame_received(&mut self, size_bytes: usize) {
        self.frames_received += 1;
        self.bytes_received += size_bytes as u64;
        self.loss_window.push_back(true);
        if self.loss_window.len() > 100 {
            self.loss_window.pop_front();
        }
        self.bandwidth_window
            .push_back((Instant::now(), size_bytes));
        if self.bandwidth_window.len() > 100 {
            self.bandwidth_window.pop_front();
        }
        self.update_bandwidth();
    }

    pub fn record_frame_dropped(&mut self) {
        self.frames_dropped += 1;
        self.loss_window.push_back(false);
        if self.loss_window.len() > 100 {
            self.loss_window.pop_front();
        }
        self.update_bandwidth();
    }

    pub fn record_latency(&mut self, latency_ms: f32) {
        self.latency_ms = latency_ms;
        self.latency_history.push_back(latency_ms);
        if self.latency_history.len() > 120 {
            self.latency_history.pop_front();
        }
    }

    fn update_bandwidth(&mut self) {
        let now = Instant::now();
        let window = Duration::from_secs(5);
        let recent: Vec<_> = self
            .bandwidth_window
            .iter()
            .filter(|(t, _)| now.duration_since(*t) < window)
            .collect();
        if !recent.is_empty() {
            let total_bytes: usize = recent.iter().map(|(_, b)| b).sum();
            let elapsed = now.duration_since(recent[0].0).as_secs_f32().max(0.1);
            self.bandwidth_kbps = (total_bytes as f32 / elapsed) / 1024.0;
        }

        let total = self.loss_window.len() as f32;
        if total > 0.0 {
            let received = self.loss_window.iter().filter(|&&r| r).count() as f32;
            self.packet_loss_pct = ((total - received) / total) * 100.0;
        }
    }

    pub fn latency_history(&self) -> &VecDeque<f32> {
        &self.latency_history
    }

    pub fn health(&self) -> ConnectionHealth {
        if self.latency_ms > 500.0 || self.packet_loss_pct > 20.0 {
            ConnectionHealth::Critical
        } else if self.latency_ms > 200.0 || self.packet_loss_pct > 5.0 {
            ConnectionHealth::Warning
        } else {
            ConnectionHealth::Good
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionHealth {
    Good,
    Warning,
    Critical,
}

impl Default for ConnectionMetrics {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ContinuumConnection {
    pub connection: Connection,
    pub server_info: Option<ServerInfo>,
    pub session_token: Option<String>,
    pub paired: bool,
    pub permissions: Permissions,
    pub ice_transport: Option<IceTransport>,
    pub shared_secret: Option<[u8; 32]>,
    pub resume_token: Option<String>,
    connected_at: Instant,
}

impl ContinuumConnection {
    pub fn latency(&self) -> Duration {
        self.connected_at.elapsed()
    }
}

pub async fn connect_to_server(config: &ClientConfig) -> Result<ContinuumConnection> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let ice_config = IceConfig {
        stun_servers: vec!["stun.l.google.com:19302".to_string()],
        turn_servers: parse_turn_servers_from_env(),
        ice_lite: false,
        nomination_mode: NominationMode::Aggressive,
        ..Default::default()
    };

    let mut ice = IceTransport::new(ice_config);
    let direct_addr = config.server_addr;
    let mut ice_used = false;
    let addr = if direct_addr.ip().is_loopback() || is_private_ip(direct_addr.ip()) {
        direct_addr
    } else {
        match ice.gather_candidates().await {
            Ok(candidates) => {
                tracing::info!(
                    count = candidates.len(),
                    "ICE candidates gathered for non-local connection"
                );
                ice_used = true;
                direct_addr
            }
            Err(e) => {
                tracing::warn!(error = %e, "ICE gathering failed, falling back to direct connection");
                direct_addr
            }
        }
    };

    let rustls_config = tls::build_client_config(None)?;
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_bidi_streams(128u32.into());
    transport_config.max_concurrent_uni_streams(64u32.into());
    transport_config.keep_alive_interval(Some(Duration::from_secs(10)));
    transport_config.max_idle_timeout(Some(
        Duration::from_secs(30)
            .try_into()
            .unwrap_or(quinn::VarInt::from_u32(30_000).into()),
    ));

    let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)?;
    let mut client_config = QuinnClientConfig::new(Arc::new(client_crypto));
    client_config.transport_config(Arc::new(transport_config));

    let endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    let connecting = endpoint.connect_with(client_config, addr, "localhost")?;
    let connection = connecting.await?;

    tracing::info!(addr = %addr, ice = ice_used, "Connected to server");

    Ok(ContinuumConnection {
        connection,
        server_info: None,
        session_token: None,
        paired: false,
        permissions: Permissions::default(),
        ice_transport: if ice_used { Some(ice) } else { None },
        shared_secret: None,
        resume_token: None,
        connected_at: Instant::now(),
    })
}

fn parse_turn_servers_from_env() -> Vec<TurnServerConfig> {
    let mut servers = Vec::new();
    if let Ok(url) = std::env::var("CONTINUUM_TURN_URL") {
        let username = std::env::var("CONTINUUM_TURN_USERNAME").unwrap_or_default();
        let credential = std::env::var("CONTINUUM_TURN_CREDENTIAL").unwrap_or_default();
        servers.push(TurnServerConfig {
            url,
            username,
            credential,
        });
    }
    servers
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_documentation()
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

pub async fn pair_connection(
    conn: &Connection,
    pairing_code: &str,
    client_name: &str,
    client_secret: &x25519_dalek::StaticSecret,
) -> Result<(PairingResponse, Option<[u8; 32]>)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;

    // Use PAKE for secure pairing
    let pake_client = continuum_security::PakeClient::new(pairing_code);
    let client_epk = pake_client.encrypted_public_key();

    let client_public = x25519_dalek::PublicKey::from(client_secret);
    let handshake = PairingHandshake {
        pairing_code: String::new(),
        client_name: client_name.to_string(),
        protocol_version: PROTOCOL_VERSION,
        client_e2e_public: client_public.as_bytes().to_vec(),
        pake_encrypted_key: client_epk,
    };
    let message = IntentMessage::Pairing(handshake);
    let payload = serde_json::to_vec(&message)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;

    let response = read_intent_response(&mut recv).await?;
    match response {
        IntentResponse::PairingResult(result) => {
            let mut shared_secret = if result.accepted
                && !result.server_e2e_public.is_empty()
                && result.server_e2e_public.len() == 32
            {
                let mut server_pub_bytes = [0u8; 32];
                server_pub_bytes.copy_from_slice(&result.server_e2e_public);
                let server_public = x25519_dalek::PublicKey::from(server_pub_bytes);
                let shared =
                    continuum_security::compute_shared_secret(client_secret, &server_public);
                tracing::info!("E2E shared secret computed on client side");
                Some(shared)
            } else {
                None
            };

            // If PAKE succeeded, derive session key from PAKE
            if result.accepted && !result.pake_encrypted_key.is_empty() {
                match pake_client.complete(&result.pake_encrypted_key) {
                    Ok(pake_result) => {
                        shared_secret = Some(pake_result.session_key);
                        tracing::info!(sas = ?pake_result.sas, "PAKE complete — verify SAS matches on both sides");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PAKE completion failed on client");
                    }
                }
            }

            Ok((result, shared_secret))
        }
        IntentResponse::Error(err) => Err(anyhow::anyhow!("Pairing error: {}", err)),
        _ => Err(anyhow::anyhow!("Unexpected pairing response")),
    }
}

pub async fn send_input_event(conn: &Connection, event: RemoteInputEvent) -> Result<()> {
    let (mut send, _recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;
    let message = IntentMessage::Input(event);
    let payload = serde_json::to_vec(&message)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;
    let _ = send.finish();
    Ok(())
}

pub async fn send_heartbeat(conn: &Connection, seq: u64) -> Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;
    let message = IntentMessage::Heartbeat(Heartbeat {
        timestamp: chrono::Utc::now(),
        sequence: seq,
    });
    let payload = serde_json::to_vec(&message)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;
    let _ = read_intent_response(&mut recv).await;
    Ok(())
}

pub async fn send_clipboard_data(conn: &Connection, data: ClipboardData) -> Result<()> {
    let (mut send, _recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;
    let message = IntentMessage::Clipboard(data);
    let payload = serde_json::to_vec(&message)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;
    let _ = send.finish();
    Ok(())
}

pub async fn send_file(conn: &Connection, path: &std::path::Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let file_size = file.metadata()?.len();
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?
        .to_string_lossy()
        .to_string();

    let hash = {
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        file.seek(SeekFrom::Start(0))?;
        format!("{:x}", hasher.finalize())
    };

    let transfer_id = format!("tx-{}", chrono::Utc::now().timestamp_millis());

    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;
    let request = IntentMessage::FileRequest(FileTransferRequest {
        filename,
        file_size,
        file_hash: hash,
        direction: TransferDirection::Upload,
    });
    let payload = serde_json::to_vec(&request)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;
    let _ = read_intent_response(&mut recv).await?;

    let chunk_size = 64 * 1024;
    let mut offset = 0u64;
    let mut buf = vec![0u8; chunk_size];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let chunk = IntentMessage::FileChunk(FileChunk {
            transfer_id: transfer_id.clone(),
            offset,
            data: buf[..n].to_vec(),
            is_last: offset + n as u64 >= file_size,
        });
        let (mut chunk_send, _chunk_recv) = conn.open_bi().await?;
        chunk_send
            .write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
            .await?;
        let payload = serde_json::to_vec(&chunk)?;
        chunk_send
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await?;
        chunk_send.write_all(&payload).await?;
        let _ = chunk_send.finish();

        offset += n as u64;
        tracing::debug!(transfer = %transfer_id, offset = offset, total = file_size, "File chunk sent");
    }

    tracing::info!(file = %path.display(), size = file_size, "File transfer complete");
    Ok(())
}

pub async fn read_frame(recv: &mut quinn::RecvStream) -> Result<(Vec<u8>, FrameSemantics)> {
    let mut len_buf = [0u8; 4];

    // Timeout: if no data for 30 seconds, connection is likely dead
    let result = tokio::time::timeout(Duration::from_secs(30), recv.read_exact(&mut len_buf)).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(anyhow::anyhow!("Frame read timed out after 30s")),
    }

    let sem_len = u32::from_be_bytes(len_buf) as usize;
    if sem_len > 1024 * 1024 {
        return Err(anyhow::anyhow!("Semantic length too large: {}", sem_len));
    }
    let mut sem_json = vec![0u8; sem_len];
    recv.read_exact(&mut sem_json).await?;
    let semantics: FrameSemantics = serde_json::from_slice(&sem_json)?;

    recv.read_exact(&mut len_buf).await?;
    let frame_len = u32::from_be_bytes(len_buf) as usize;
    if frame_len > 64 * 1024 * 1024 {
        return Err(anyhow::anyhow!("Frame length too large: {}", frame_len));
    }
    let mut frame_data = vec![0u8; frame_len];
    recv.read_exact(&mut frame_data).await?;
    Ok((frame_data, semantics))
}

pub async fn read_intent_response(recv: &mut quinn::RecvStream) -> Result<IntentResponse> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut data = vec![0u8; len];
    recv.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}

pub async fn try_resume_connection(
    conn: &Connection,
    resume_token: &str,
    client_name: &str,
) -> Result<(PairingResponse, bool)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&(ApqStreamType::Intent as u32).to_be_bytes())
        .await?;

    let message = IntentMessage::Resume(ResumeRequest {
        token: resume_token.to_string(),
        client_name: client_name.to_string(),
    });
    let payload = serde_json::to_vec(&message)?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&payload).await?;

    let response = read_intent_response(&mut recv).await?;
    match response {
        IntentResponse::PairingResult(result) if result.accepted => Ok((result, true)),
        IntentResponse::PairingResult(result) => Ok((result, false)),
        IntentResponse::Error(err) => Err(anyhow::anyhow!("Resume error: {}", err)),
        _ => Err(anyhow::anyhow!("Unexpected resume response")),
    }
}

pub struct ReconnectPolicy {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter: bool,
    current_delay: Duration,
    attempt: u32,
}

impl ReconnectPolicy {
    pub fn new(max_delay: Duration) -> Self {
        Self {
            base_delay: Duration::from_millis(500),
            max_delay,
            multiplier: 1.5,
            jitter: true,
            current_delay: Duration::from_millis(500),
            attempt: 0,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        self.attempt += 1;
        let delay = self.current_delay;
        let mut next = self.current_delay.mul_f64(self.multiplier);
        if next > self.max_delay {
            next = self.max_delay;
        }
        if self.jitter {
            let jitter_range = next.as_millis() as u64 / 4;
            if jitter_range > 0 {
                next += Duration::from_millis(rand::random::<u64>() % jitter_range);
            }
        }
        self.current_delay = next;
        delay
    }

    pub fn reset(&mut self) {
        self.current_delay = self.base_delay;
        self.attempt = 0;
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

pub async fn maintain_connection(
    config: &ClientConfig,
    event_tx: mpsc::Sender<ConnectionEvent>,
    mut command_rx: mpsc::Receiver<ControlCommand>,
) -> Result<()> {
    let mut reconnect = ReconnectPolicy::default();
    let (client_secret, _client_public) = continuum_security::generate_dh_keypair();
    let mut last_resume_token: Option<String> = None;

    loop {
        let _ = event_tx
            .send(ConnectionEvent::Status(ConnectionStatus::Connecting))
            .await;

        match connect_to_server(config).await {
            Ok(mut conn) => {
                reconnect.reset();
                let _ = event_tx
                    .send(ConnectionEvent::Status(ConnectionStatus::Connected))
                    .await;

                let pair_result = if let Some(ref token) = last_resume_token {
                    match try_resume_connection(&conn.connection, token, &config.client_name).await
                    {
                        Ok((response, true)) => {
                            tracing::info!("Session resumed successfully");
                            Ok((response, conn.shared_secret))
                        }
                        Ok((_response, false)) => {
                            tracing::warn!("Resume rejected, falling back to full pairing");
                            last_resume_token = None;
                            pair_connection(
                                &conn.connection,
                                &config.pairing_code,
                                &config.client_name,
                                &client_secret,
                            )
                            .await
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "Resume failed, falling back to full pairing");
                            last_resume_token = None;
                            pair_connection(
                                &conn.connection,
                                &config.pairing_code,
                                &config.client_name,
                                &client_secret,
                            )
                            .await
                        }
                    }
                } else {
                    pair_connection(
                        &conn.connection,
                        &config.pairing_code,
                        &config.client_name,
                        &client_secret,
                    )
                    .await
                };

                match pair_result {
                    Ok((response, shared_secret)) if response.accepted => {
                        conn.paired = true;
                        conn.session_token = response.session_token.clone();
                        conn.permissions = response.permissions.clone();
                        conn.shared_secret = shared_secret;
                        conn.resume_token = response.resume_token.clone();
                        last_resume_token = response.resume_token.clone();
                        let _ = event_tx
                            .send(ConnectionEvent::Status(ConnectionStatus::Paired))
                            .await;
                        let _ = event_tx
                            .send(ConnectionEvent::PairingResult(response))
                            .await;

                        let (mut media_send, mut media_recv) = conn.connection.open_bi().await?;
                        media_send
                            .write_all(&(ApqStreamType::Media as u32).to_be_bytes())
                            .await?;

                        // Open audio stream
                        let (mut audio_send, mut audio_recv) = conn.connection.open_bi().await?;
                        audio_send
                            .write_all(&(ApqStreamType::Audio as u32).to_be_bytes())
                            .await?;
                        let audio_tx = event_tx.clone();
                        tokio::spawn(async move {
                            loop {
                                let mut len_buf = [0u8; 4];
                                match audio_recv.read_exact(&mut len_buf).await {
                                    Ok(()) => {}
                                    Err(_) => break,
                                }
                                let len = u32::from_be_bytes(len_buf) as usize;
                                if len > 1024 * 1024 {
                                    break;
                                }
                                let mut data = vec![0u8; len];
                                if audio_recv.read_exact(&mut data).await.is_err() {
                                    break;
                                }
                                if let Ok(frame) =
                                    serde_json::from_slice::<crate::audio::AudioFrame>(&data)
                                {
                                    let _ = audio_tx.send(ConnectionEvent::Audio(frame)).await;
                                }
                            }
                        });

                        let _ = event_tx
                            .send(ConnectionEvent::Status(ConnectionStatus::Streaming))
                            .await;

                        let mut e2e_decryptor = E2EEncryptor::new();
                        if let Some(secret) = shared_secret {
                            e2e_decryptor.init_with_secret(secret);
                            tracing::info!("E2E decryption active on client");
                        }

                        let mut heartbeat_seq = 0u64;
                        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(5));
                        let mut metrics = ConnectionMetrics::new();
                        let mut metrics_interval = tokio::time::interval(Duration::from_secs(2));

                        loop {
                            tokio::select! {
                                _ = heartbeat_interval.tick() => {
                                    heartbeat_seq += 1;
                                    let _ = send_heartbeat(&conn.connection, heartbeat_seq).await;
                                }
                                _ = metrics_interval.tick() => {
                                    let _ = event_tx.send(ConnectionEvent::Metrics(metrics.clone())).await;
                                }
                                cmd = command_rx.recv() => {
                                    match cmd {
                                        Some(ControlCommand::Input(event)) => {
                                            if conn.permissions.can_control {
                                                let _ = send_input_event(&conn.connection, event).await;
                                            }
                                        }
                                        Some(ControlCommand::Clipboard(data)) => {
                                            if conn.permissions.can_clipboard {
                                                let _ = send_clipboard_data(&conn.connection, data).await;
                                            }
                                        }
                                        Some(ControlCommand::Pair(_)) => {}
                                        Some(ControlCommand::Disconnect) => {
                                            conn.connection.close(VarInt::from_u32(0), b"disconnect");
                                            return Ok(());
                                        }
                                        None => {
                                            conn.connection.close(VarInt::from_u32(0), b"closed");
                                            return Ok(());
                                        }
                                    }
                                }
                                incoming = conn.connection.accept_bi() => {
                                    match incoming {
                                        Ok((mut send, mut recv)) => {
                                            let mut hdr = [0u8; 4];
                                            if recv.read_exact(&mut hdr).await.is_ok() {
                                                let stream_type = ApqStreamType::from(u32::from_be_bytes(hdr));
                                                match stream_type {
                                                    ApqStreamType::Clipboard => {
                                                        let mut len_buf = [0u8; 4];
                                                        if recv.read_exact(&mut len_buf).await.is_ok() {
                                                            let len = u32::from_be_bytes(len_buf) as usize;
                                                            if len <= 16 * 1024 * 1024 {
                                                                let mut data = vec![0u8; len];
                                                                if recv.read_exact(&mut data).await.is_ok() {
                                                                    if let Ok(clipboard) = serde_json::from_slice::<ClipboardData>(&data) {
                                                                        let _ = event_tx.send(ConnectionEvent::Clipboard(clipboard)).await;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        let _ = send.finish();
                                                    }
                                                    _ => { let _ = send.finish(); }
                                                }
                                            }
                                        }
                                        Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                                        Err(_) => {}
                                    }
                                }
                                frame = read_frame(&mut media_recv) => {
                                    match frame {
                                        Ok((data, semantics)) => {
                                            metrics.record_frame_received(data.len());
                                            let decrypted = if e2e_decryptor.is_active() {
                                                match e2e_decryptor.decrypt_frame(&data) {
                                                    Ok(d) => d,
                                                    Err(e) => {
                                                        tracing::error!(error = %e, "E2E decrypt failed — dropping frame");
                                                        continue;
                                                    }
                                                }
                                            } else {
                                                data
                                            };
                                            let _ = event_tx.send(ConnectionEvent::Frame { data: decrypted, semantics }).await;
                                        }
                                        Err(err) => {
                                            metrics.record_frame_dropped();
                                            let _ = event_tx.send(ConnectionEvent::Error(err.to_string())).await;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok((response, _shared_secret)) => {
                        let _ = event_tx
                            .send(ConnectionEvent::PairingResult(response))
                            .await;
                        let _ = event_tx
                            .send(ConnectionEvent::Status(ConnectionStatus::Disconnected))
                            .await;
                    }
                    Err(err) => {
                        let _ = event_tx.send(ConnectionEvent::Error(err.to_string())).await;
                    }
                }
            }
            Err(err) => {
                let _ = event_tx.send(ConnectionEvent::Error(err.to_string())).await;
            }
        }

        if !config.auto_reconnect {
            let _ = event_tx
                .send(ConnectionEvent::Status(ConnectionStatus::Disconnected))
                .await;
            return Ok(());
        }

        let delay = reconnect.next_delay();
        tracing::info!(
            delay_ms = delay.as_millis(),
            attempt = reconnect.attempt(),
            "Reconnecting"
        );
        let _ = event_tx
            .send(ConnectionEvent::Status(ConnectionStatus::Reconnecting))
            .await;
        tokio::time::sleep(delay).await;
    }
}

#[derive(Debug)]
pub enum ConnectionEvent {
    Frame {
        data: Vec<u8>,
        semantics: FrameSemantics,
    },
    Audio(crate::audio::AudioFrame),
    Status(ConnectionStatus),
    Metrics(ConnectionMetrics),
    PairingResult(PairingResponse),
    Clipboard(ClipboardData),
    Error(String),
}

#[derive(Debug)]
pub enum ControlCommand {
    Pair(PairingHandshake),
    Input(RemoteInputEvent),
    Clipboard(ClipboardData),
    Disconnect,
}
