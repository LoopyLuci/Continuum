use crate::capture::enumerate_monitors;
use crate::config::ServerConfig;
use crate::datagram::DatagramHandler;
use crate::e2e::E2EEncryptor;
use crate::gpu_encoder::{create_encoder, detect_best_encoder, VideoEncoder};
use crate::input::InputInjector;
use crate::plugin::NativePluginRegistry;
use crate::recording::SessionRecorder;
use crate::streaming_engine::{RateControlConfig, RateControlSample, RateController};
use crate::tls;
use crate::types::*;
use anyhow::{Context, Result};
use chrono::Utc;
use continuum_observability::audit::{AuditEvent, AuditLogger};
use continuum_observability::health::HealthServer;
use continuum_observability::telemetry::MetricsRegistry;
use parking_lot::RwLock;
use quinn::{Connection, Endpoint, RecvStream, SendStream, VarInt};
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

struct RateLimiter {
    attempts: Vec<Instant>,
    max_attempts: u32,
    window: std::time::Duration,
}

impl RateLimiter {
    fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            attempts: Vec::new(),
            max_attempts,
            window: std::time::Duration::from_secs(window_secs),
        }
    }

    fn check(&mut self) -> bool {
        let now = Instant::now();
        self.attempts
            .retain(|t| now.duration_since(*t) < self.window);
        if self.attempts.len() as u32 >= self.max_attempts {
            false
        } else {
            self.attempts.push(now);
            true
        }
    }
}

#[derive(Clone)]
struct ResumableSession {
    session_id: String,
    shared_secret: Option<[u8; 32]>,
    paired: bool,
    permissions: Permissions,
    created_at: Instant,
}

struct ClientState {
    paired: bool,
    permissions: Permissions,
    rate_limiter: RateLimiter,
    session_id: String,
    session_token: String,
    #[allow(dead_code)]
    connected_at: Instant,
    frames_sent: u64,
    shared_secret: Option<[u8; 32]>,
}

struct ServerState {
    clients: RwLock<HashMap<SocketAddr, ClientState>>,
    pairing_code: String,
    max_clients: usize,
    connection_counter: AtomicU64,
    #[allow(dead_code)]
    monitors: Vec<MonitorInfo>,
    selected_monitor: parking_lot::RwLock<u32>,
    metrics: Arc<MetricsRegistry>,
    audit: AuditLogger,
    recorder: parking_lot::Mutex<SessionRecorder>,
    plugins: NativePluginRegistry,
    last_clipboard: RwLock<Option<ClipboardData>>,
    clipboard_update_tx: tokio::sync::broadcast::Sender<ClipboardData>,
    pub resume_tokens: RwLock<HashMap<String, ResumableSession>>,
    pub partial_transfers: RwLock<HashMap<String, PartialTransfer>>,
    #[allow(dead_code)]
    anomaly_detector: parking_lot::Mutex<InlineAnomalyDetector>,
    require_pake: bool,
}

#[derive(Debug, Clone)]
pub struct PartialTransfer {
    pub filename: String,
    pub total_size: u64,
    pub received_bytes: u64,
    pub chunks_received: Vec<u64>,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct AnomalyWarning {
    metric: String,
    value: f64,
    mean: f64,
    std_dev: f64,
    sigma: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct InlineAnomalyDetector {
    encode_times: std::collections::VecDeque<f64>,
    frame_sizes: std::collections::VecDeque<f64>,
    window_size: usize,
    threshold_sigma: f64,
    warnings: Vec<AnomalyWarning>,
}

#[allow(dead_code)]
impl InlineAnomalyDetector {
    fn new(window_size: usize, threshold_sigma: f64) -> Self {
        Self {
            encode_times: std::collections::VecDeque::with_capacity(window_size),
            frame_sizes: std::collections::VecDeque::with_capacity(window_size),
            window_size,
            threshold_sigma,
            warnings: Vec::new(),
        }
    }

    fn record_encode_time(&mut self, time_us: f64) {
        self.encode_times.push_back(time_us);
        if self.encode_times.len() > self.window_size {
            self.encode_times.pop_front();
        }
        self.check_anomaly("encode_time", time_us);
    }

    fn record_frame_size(&mut self, size_bytes: f64) {
        self.frame_sizes.push_back(size_bytes);
        if self.frame_sizes.len() > self.window_size {
            self.frame_sizes.pop_front();
        }
        self.check_anomaly("frame_size", size_bytes);
    }

    fn check_anomaly(&mut self, metric: &str, value: f64) {
        let data: &std::collections::VecDeque<f64> = match metric {
            "encode_time" => &self.encode_times,
            "frame_size" => &self.frame_sizes,
            _ => return,
        };

        if data.len() < 10 {
            return;
        }

        let mean = data.iter().sum::<f64>() / data.len() as f64;
        let variance =
            data.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / data.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 1e-10 {
            return;
        }

        let sigma = (value - mean).abs() / std_dev;

        if sigma > self.threshold_sigma {
            tracing::warn!(
                "Anomaly detected: {} = {:.1} (mean={:.1}, σ={:.1}, {:.1}σ deviation)",
                metric,
                value,
                mean,
                std_dev,
                sigma
            );
            self.warnings.push(AnomalyWarning {
                metric: metric.to_string(),
                value,
                mean,
                std_dev,
                sigma,
            });
        }
    }

    #[allow(dead_code)]
    fn warnings(&self) -> &[AnomalyWarning] {
        &self.warnings
    }

    #[allow(dead_code)]
    fn clear_warnings(&mut self) {
        self.warnings.clear();
    }

    #[allow(dead_code)]
    fn mean_encode_time(&self) -> f64 {
        if self.encode_times.is_empty() {
            return 0.0;
        }
        self.encode_times.iter().sum::<f64>() / self.encode_times.len() as f64
    }

    #[allow(dead_code)]
    fn mean_frame_size(&self) -> f64 {
        if self.frame_sizes.is_empty() {
            return 0.0;
        }
        self.frame_sizes.iter().sum::<f64>() / self.frame_sizes.len() as f64
    }

    #[allow(dead_code)]
    fn is_anomaly(&self, metric: &str, value: f64) -> bool {
        let data: &std::collections::VecDeque<f64> = match metric {
            "encode_time" => &self.encode_times,
            "frame_size" => &self.frame_sizes,
            _ => return false,
        };

        if data.len() < 10 {
            return false;
        }

        let mean = data.iter().sum::<f64>() / data.len() as f64;
        let variance =
            data.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / data.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 1e-10 {
            return false;
        }

        let sigma = (value - mean).abs() / std_dev;
        sigma > self.threshold_sigma
    }
}

impl Default for InlineAnomalyDetector {
    fn default() -> Self {
        Self::new(100, 3.0)
    }
}

impl ServerState {
    fn new(config: &ServerConfig) -> Self {
        let output_dir = config
            .record_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .map(|d| d.join("continuum").join("recordings"))
                    .unwrap_or_else(|| PathBuf::from("./recordings"))
            });
        let audit = if let Some(ref path) = config.audit_frames {
            match AuditLogger::new_with_file(path) {
                Ok(logger) => {
                    tracing::info!(path = %path.display(), "Frame audit logging enabled");
                    logger
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to open audit file, falling back to in-memory");
                    AuditLogger::new()
                }
            }
        } else {
            AuditLogger::new()
        };
        let (clipboard_tx, _) = tokio::sync::broadcast::channel(16);
        Self {
            clients: RwLock::new(HashMap::new()),
            pairing_code: config.pairing_code.clone(),
            max_clients: config.max_clients,
            connection_counter: AtomicU64::new(0),
            monitors: enumerate_monitors(),
            selected_monitor: parking_lot::RwLock::new(0),
            metrics: Arc::new(MetricsRegistry::new()),
            audit,
            recorder: parking_lot::Mutex::new(SessionRecorder::new(output_dir)),
            plugins: NativePluginRegistry::default(),
            last_clipboard: RwLock::new(None),
            clipboard_update_tx: clipboard_tx,
            resume_tokens: RwLock::new(HashMap::new()),
            partial_transfers: RwLock::new(HashMap::new()),
            anomaly_detector: parking_lot::Mutex::new(InlineAnomalyDetector::default()),
            require_pake: config.require_pake,
        }
    }

    fn register_client(&self, addr: SocketAddr) -> bool {
        let mut clients = self.clients.write();
        if clients.len() >= self.max_clients {
            return false;
        }
        let session_id = format!(
            "sess-{}",
            self.connection_counter.fetch_add(1, Ordering::Relaxed)
        );
        let session_token = Self::generate_session_token();
        self.metrics
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
        clients.insert(
            addr,
            ClientState {
                paired: false,
                permissions: Permissions::default(),
                rate_limiter: RateLimiter::new(5, 60),
                session_id,
                session_token,
                connected_at: Instant::now(),
                frames_sent: 0,
                shared_secret: None,
            },
        );
        self.audit.log(
            AuditEvent::ConnectionOpened,
            &addr.to_string(),
            &clients
                .get(&addr)
                .map(|c| c.session_id.clone())
                .unwrap_or_default(),
            "Connection established",
        );
        true
    }

    fn unregister_client(&self, addr: &SocketAddr) {
        let session_id = self
            .clients
            .read()
            .get(addr)
            .map(|c| c.session_id.clone())
            .unwrap_or_default();
        self.clients.write().remove(addr);
        self.metrics
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.audit.log(
            AuditEvent::ConnectionClosed,
            &addr.to_string(),
            &session_id,
            "Connection closed",
        );
    }

    fn pair_client(
        &self,
        addr: &SocketAddr,
        code: &str,
        client_name: &str,
        client_e2e_public: &[u8],
        pake_encrypted_key: &[u8],
    ) -> PairingResponse {
        let mut clients = self.clients.write();
        if let Some(client) = clients.get_mut(addr) {
            if !client.rate_limiter.check() {
                self.audit.log(
                    AuditEvent::PairingAttempt { success: false },
                    &addr.to_string(),
                    &client.session_id,
                    "Rate limited",
                );
                return PairingResponse {
                    accepted: false,
                    message: "Rate limited. Try again later.".to_string(),
                    session_token: None,
                    permissions: Permissions::default(),
                    server_e2e_public: Vec::new(),
                    pake_encrypted_key: Vec::new(),
                    resume_token: None,
                    sas_words: Vec::new(),
                };
            }

            // PAKE mode: derive shared secret from encrypted keys
            if !pake_encrypted_key.is_empty() {
                let server_pake = continuum_security::PakeServer::new(&self.pairing_code);
                let server_epk = server_pake.encrypted_public_key();

                match server_pake.complete(pake_encrypted_key) {
                    Ok(result) => {
                        client.paired = true;
                        client.shared_secret = Some(result.session_key);
                        let token = Self::generate_session_token();
                        let old_token = std::mem::replace(&mut client.session_token, token.clone());

                        self.audit.log(
                            AuditEvent::PairingAttempt { success: true },
                            &addr.to_string(),
                            &client.session_id,
                            &format!("Paired as {} (PAKE)", client_name),
                        );
                        tracing::info!(addr = %addr, client = %client_name, "Client paired via PAKE");

                        let resume_token = Self::generate_resume_token();
                        self.resume_tokens.write().insert(
                            resume_token.clone(),
                            ResumableSession {
                                session_id: client.session_id.clone(),
                                shared_secret: client.shared_secret,
                                paired: true,
                                permissions: client.permissions.clone(),
                                created_at: Instant::now(),
                            },
                        );

                        return PairingResponse {
                            accepted: true,
                            message: "Pairing successful (PAKE)".to_string(),
                            session_token: Some(old_token),
                            permissions: client.permissions.clone(),
                            server_e2e_public: Vec::new(),
                            pake_encrypted_key: server_epk,
                            resume_token: Some(resume_token),
                            sas_words: result.sas.to_vec(),
                        };
                    }
                    Err(e) => {
                        self.audit.log(
                            AuditEvent::PairingAttempt { success: false },
                            &addr.to_string(),
                            &client.session_id,
                            &format!("PAKE failed: {}", e),
                        );
                        tracing::warn!(addr = %addr, error = %e, "PAKE pairing failed");
                        return PairingResponse {
                            accepted: false,
                            message: format!("PAKE failed: {}", e),
                            session_token: None,
                            permissions: Permissions::default(),
                            server_e2e_public: Vec::new(),
                            pake_encrypted_key: Vec::new(),
                            resume_token: None,
                            sas_words: Vec::new(),
                        };
                    }
                }
            }

            if self.require_pake {
                self.audit.log(
                    AuditEvent::PairingAttempt { success: false },
                    &addr.to_string(),
                    &client.session_id,
                    "PAKE required but not provided",
                );
                tracing::warn!(addr = %addr, "Rejecting non-PAKE pairing because require_pake is enabled");
                return PairingResponse {
                    accepted: false,
                    message: "PAKE pairing is required".to_string(),
                    session_token: None,
                    permissions: Permissions::default(),
                    server_e2e_public: Vec::new(),
                    pake_encrypted_key: Vec::new(),
                    resume_token: None,
                    sas_words: Vec::new(),
                };
            }

            // Legacy pairing code comparison
            let code_bytes = code.as_bytes();
            let expected_bytes = self.pairing_code.as_bytes();
            let code_matches =
                code_bytes.len() == expected_bytes.len() && code_bytes.ct_eq(expected_bytes).into();
            if code_matches {
                client.paired = true;
                let token = Self::generate_session_token();
                let old_token = std::mem::replace(&mut client.session_token, token.clone());

                // DH key exchange
                let (server_secret, server_public) = continuum_security::generate_dh_keypair();
                let mut server_e2e_public = server_public.as_bytes().to_vec();

                if !client_e2e_public.is_empty() && client_e2e_public.len() == 32 {
                    let mut client_pub_bytes = [0u8; 32];
                    client_pub_bytes.copy_from_slice(client_e2e_public);
                    let client_public = x25519_dalek::PublicKey::from(client_pub_bytes);
                    let shared =
                        continuum_security::compute_shared_secret(&server_secret, &client_public);
                    client.shared_secret = Some(shared);
                    tracing::info!(addr = %addr, "E2E shared secret computed");
                } else {
                    server_e2e_public.clear();
                    tracing::info!(addr = %addr, "No E2E public key from client, skipping E2E setup");
                }

                self.audit.log(
                    AuditEvent::PairingAttempt { success: true },
                    &addr.to_string(),
                    &client.session_id,
                    &format!("Paired as {}", client_name),
                );
                tracing::info!(addr = %addr, client = %client_name, "Client paired");
                let resume_token = Self::generate_resume_token();
                self.resume_tokens.write().insert(
                    resume_token.clone(),
                    ResumableSession {
                        session_id: client.session_id.clone(),
                        shared_secret: client.shared_secret,
                        paired: true,
                        permissions: client.permissions.clone(),
                        created_at: Instant::now(),
                    },
                );
                PairingResponse {
                    accepted: true,
                    message: "Pairing successful".to_string(),
                    session_token: Some(old_token),
                    permissions: client.permissions.clone(),
                    server_e2e_public,
                    pake_encrypted_key: Vec::new(),
                    resume_token: Some(resume_token),
                    sas_words: Vec::new(),
                }
            } else {
                self.audit.log(
                    AuditEvent::PairingAttempt { success: false },
                    &addr.to_string(),
                    &client.session_id,
                    "Wrong pairing code",
                );
                tracing::warn!(addr = %addr, "Pairing attempt with wrong code");
                PairingResponse {
                    accepted: false,
                    message: "Invalid pairing code".to_string(),
                    session_token: None,
                    permissions: Permissions::default(),
                    server_e2e_public: Vec::new(),
                    pake_encrypted_key: Vec::new(),
                    resume_token: None,
                    sas_words: Vec::new(),
                }
            }
        } else {
            PairingResponse {
                accepted: false,
                message: "Not registered".to_string(),
                session_token: None,
                permissions: Permissions::default(),
                server_e2e_public: Vec::new(),
                pake_encrypted_key: Vec::new(),
                resume_token: None,
                sas_words: Vec::new(),
            }
        }
    }

    fn generate_session_token() -> String {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    }

    fn generate_resume_token() -> String {
        let mut bytes = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    }

    fn can_control(&self, addr: &SocketAddr) -> bool {
        self.clients
            .read()
            .get(addr)
            .map(|c| c.paired && c.permissions.can_control)
            .unwrap_or(false)
    }

    fn session_id(&self, addr: &SocketAddr) -> String {
        self.clients
            .read()
            .get(addr)
            .map(|c| c.session_id.clone())
            .unwrap_or_default()
    }
    fn is_paired(&self, addr: &SocketAddr) -> bool {
        self.clients
            .read()
            .get(addr)
            .map(|c| c.paired)
            .unwrap_or(false)
    }

    fn increment_frames(&self, addr: &SocketAddr) {
        if let Some(client) = self.clients.write().get_mut(addr) {
            client.frames_sent += 1;
        }
    }

    fn evict_expired_resume_tokens(&self) {
        let cutoff = Instant::now() - Duration::from_secs(3600);
        let mut tokens = self.resume_tokens.write();
        tokens.retain(|_, session| session.created_at > cutoff);
    }

    fn get_shared_secret(&self, addr: &SocketAddr) -> Option<[u8; 32]> {
        self.clients.read().get(addr).and_then(|c| c.shared_secret)
    }

    pub fn available_monitors(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    fn shutdown(&self) {
        let mut recorder = self.recorder.lock();
        if let Some(record) = recorder.stop_session() {
            tracing::info!(session = %record.session_id, "Recording finalized on shutdown");
        }
        drop(recorder);

        self.audit.log(
            AuditEvent::SessionEnded,
            "server",
            "system",
            "Server shutting down",
        );

        tracing::info!("Server state cleaned up");
    }
}

pub async fn run_server(config: ServerConfig) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert = tls::load_or_generate_cert(config.cert_file.as_deref(), config.key_file.as_deref())?;
    tracing::info!(fingerprint = %cert.fingerprint, "TLS certificate loaded");

    let rustls_config = tls::build_server_config(cert)?;
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_bidi_streams(128u32.into());
    transport_config.max_concurrent_uni_streams(64u32.into());
    transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    transport_config.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .unwrap_or(quinn::VarInt::from_u32(30_000).into()),
    ));

    let server_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_config)?;
    let mut server_config_quic = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
    server_config_quic.transport_config(Arc::new(transport_config));

    let endpoint = Endpoint::server(server_config_quic, config.listen_addr)
        .context("Failed to create QUIC endpoint")?;

    tracing::info!(addr = %config.listen_addr, "QUIC endpoint bound successfully");

    let state = Arc::new(ServerState::new(&config));

    // Periodic cleanup of expired resume tokens so the map does not grow unbounded.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                state.evict_expired_resume_tokens();
            }
        });
    }

    let health_server = HealthServer::new();
    let health_addr = "127.0.0.1:9091".to_string();
    let hs = health_server.app();
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(&health_addr).await {
            Ok(listener) => {
                tracing::info!(addr = %health_addr, "Health server started");
                if let Err(e) = axum::serve(listener, hs).await {
                    tracing::error!(error = %e, "Health server failed");
                }
            }
            Err(e) => {
                tracing::error!(addr = %health_addr, error = %e, "Failed to bind health server");
            }
        }
    });

    let encoder_backend = detect_best_encoder();
    tracing::info!(encoder = %encoder_backend.name(), "Using encoder backend");

    let encoder = Arc::new(parking_lot::Mutex::new(create_encoder(encoder_backend)));
    let rate_control = Arc::new(parking_lot::Mutex::new(RateController::new(
        RateControlConfig {
            target_fps: config.target_fps,
            ..Default::default()
        },
    )));

    tracing::info!(addr = %config.listen_addr, "Server started");

    // Broadcast presence on LAN so clients can auto-discover
    {
        let server_name = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "Continuum Host".into());
        let listen_port = config.listen_addr.port();
        tokio::spawn(async move {
            let broadcast_addr: SocketAddr = match "255.255.255.255:9999".parse() {
                Ok(a) => a,
                Err(_) => return,
            };
            let socket = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to bind broadcast socket");
                    return;
                }
            };
            let _ = socket.set_broadcast(true);

            loop {
                let announcement = serde_json::json!({
                    "type": "continuum-announce",
                    "name": server_name,
                    "port": listen_port,
                    "version": env!("CARGO_PKG_VERSION"),
                });
                if let Ok(data) = serde_json::to_vec(&announcement) {
                    let _ = socket.send_to(&data, broadcast_addr).await;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }

    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        let encoder = encoder.clone();
        let rate_control = rate_control.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let addr = conn.remote_address();
                    if !state.register_client(addr) {
                        conn.close(VarInt::from_u32(1), b"server full");
                        return;
                    }
                    state.audit.log(
                        AuditEvent::ConnectionOpened,
                        &addr.to_string(),
                        &state.session_id(&addr),
                        "New QUIC connection",
                    );
                    tracing::info!(addr = %addr, "New connection");
                    if let Err(err) =
                        handle_connection(conn, state.clone(), encoder, rate_control).await
                    {
                        tracing::error!(addr = %addr, error = %err, "Connection error");
                        state.metrics.errors.fetch_add(1, Ordering::Relaxed);
                    }
                    state.unregister_client(&addr);
                    tracing::info!(addr = %addr, "Connection closed");
                }
                Err(err) => tracing::error!(error = %err, "Failed to accept connection"),
            }
        });
    }

    state.shutdown();
    tracing::info!("Server shutdown complete");
    Ok(())
}

async fn handle_connection(
    conn: Connection,
    state: Arc<ServerState>,
    encoder: Arc<parking_lot::Mutex<Box<dyn VideoEncoder>>>,
    rate_control: Arc<parking_lot::Mutex<RateController>>,
) -> Result<()> {
    let addr = conn.remote_address();
    let mut datagram_handler = DatagramHandler::new();

    // Create InputInjector ONCE per connection, not per event
    let mut injector = match InputInjector::new() {
        Ok(i) => Some(i),
        Err(e) => {
            tracing::error!(addr = %addr, error = %e, "Failed to initialize input injector");
            None
        }
    };

    let mut clipboard_rx = state.clipboard_update_tx.subscribe();

    loop {
        tokio::select! {
            Ok(clipboard) = clipboard_rx.recv() => {
                if state.is_paired(&addr) {
                    if let Ok((mut send, mut recv)) = conn.open_bi().await {
                        send.write_all(&(ApqStreamType::Clipboard as u32).to_be_bytes()).await.ok();
                        if let Ok(payload) = serde_json::to_vec(&clipboard) {
                            send.write_all(&(payload.len() as u32).to_be_bytes()).await.ok();
                            send.write_all(&payload).await.ok();
                        }
                        let _ = send.finish();
                        let _ = recv.stop(VarInt::from_u32(0));
                    }
                }
            }
            datagram = conn.read_datagram() => {
                match datagram {
                    Ok(data) => {
                        if let Ok(Some(event)) = datagram_handler.process_datagram(&data) {
                            let session_id = state.session_id(&addr);
                            state.audit.log(AuditEvent::InputEvent { action: format!("{:?}", event.action) }, &addr.to_string(), &session_id, "Datagram input");
                            if state.can_control(&addr) {
                                if let Some(ref mut inj) = injector {
                                    if let Err(err) = inj.apply(&event) {
                                        tracing::warn!(addr = %addr, error = %err, "Input injection failed");
                                        state.metrics.errors.fetch_add(1, Ordering::Relaxed);
                                    }
                                } else {
                                    tracing::warn!(addr = %addr, "Input injection unavailable");
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            stream = conn.accept_bi() => {
                match stream {
                    Ok((send, recv)) => {
                        let state = state.clone();
                        let encoder = encoder.clone();
                        let rate_control = rate_control.clone();
                        let metrics = state.metrics.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_bi_stream(send, recv, state, encoder, rate_control, metrics, addr).await {
                                tracing::debug!(addr = %addr, error = %err, "Stream error");
                            }
                        });
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(err) => {
                        tracing::error!(addr = %addr, error = %err, "Connection error");
                        break;
                    }
                }
            }
        }
    }

    // Signal disconnect to client by sending a close frame
    conn.close(VarInt::from_u32(0), b"server shutdown");
    Ok(())
}

async fn handle_bi_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    state: Arc<ServerState>,
    encoder: Arc<parking_lot::Mutex<Box<dyn VideoEncoder>>>,
    rate_control: Arc<parking_lot::Mutex<RateController>>,
    metrics: Arc<MetricsRegistry>,
    addr: SocketAddr,
) -> Result<()> {
    let mut header = [0u8; 4];
    if recv.read_exact(&mut header).await.is_err() {
        return Ok(());
    }
    let stream_type = ApqStreamType::from(u32::from_be_bytes(header));

    match stream_type {
        ApqStreamType::Media => {
            handle_media_stream(&mut send, state, encoder, rate_control, metrics, addr).await
        }
        ApqStreamType::Intent => handle_intent_stream(&mut send, &mut recv, state, addr).await,
        ApqStreamType::Audio => handle_audio_stream(&mut send, state, addr).await,
        ApqStreamType::Debug => handle_debug_stream(&mut send, &mut recv, state, addr).await,
        ApqStreamType::Clipboard => handle_clipboard_stream(&mut send, &mut recv, state, addr).await,
        ApqStreamType::FileTransfer => handle_file_transfer_stream(&mut send, &mut recv, state, addr).await,
    }
}

async fn handle_media_stream(
    send: &mut SendStream,
    state: Arc<ServerState>,
    encoder: Arc<parking_lot::Mutex<Box<dyn VideoEncoder>>>,
    rate_control: Arc<parking_lot::Mutex<RateController>>,
    metrics: Arc<MetricsRegistry>,
    addr: SocketAddr,
) -> Result<()> {
    if !state.is_paired(&addr) {
        tracing::warn!(addr = %addr, "Unpaired media stream");
        return Ok(());
    }

    let session_id = state.session_id(&addr);
    let monitor_id = *state.selected_monitor.read();
    let mut e2e = E2EEncryptor::new();
    if let Some(secret) = state.get_shared_secret(&addr) {
        e2e.init_with_secret(secret);
        tracing::info!(addr = %addr, "E2E encryption active for this session");
    } else {
        tracing::warn!(addr = %addr, "No E2E shared secret — frames unencrypted");
    }

    // Start recording for this session
    {
        let mut recorder = state.recorder.lock();
        let _ = recorder.start_session();
        tracing::info!(addr = %addr, session = %session_id, "Recording started");
    }

    // Notify plugins that session has started
    state.plugins.on_session_start(&session_id);

    loop {
        let decision = rate_control.lock().decide();
        if decision.skip_frame {
            let interval_ms = (1000.0 / decision.fps as f32) as u64;
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            continue;
        }

        let (mut payload, mut semantics) = {
            let start = Instant::now();
            match crate::capture::capture_monitor(monitor_id) {
                Ok(img) => {
                    let is_keyframe = decision.should_send_keyframe;
                    let quality = decision.quality;

                    let mut enc = encoder.lock();
                    match enc.encode(&img, is_keyframe, quality) {
                        Ok(frame) => {
                            let encode_time = start.elapsed().as_micros() as u64;
                            metrics.record_encode(encode_time, frame.data.len() as u64);

                            rate_control.lock().record_sample(RateControlSample {
                                timestamp: Utc::now(),
                                encode_time_us: encode_time,
                                frame_size_bytes: frame.data.len() as u32,
                                rtt_ms: 0.0,
                                client_decode_time_us: None,
                                bandwidth_estimate_kbps: 10000.0,
                                packet_loss: 0.0,
                            });

                            state.anomaly_detector.lock().record_encode_time(encode_time as f64);
                            state.anomaly_detector.lock().record_frame_size(frame.data.len() as f64);

                            (
                                frame.data,
                                FrameSemantics {
                                    content_type: ContentType::Jpeg,
                                    width: frame.width,
                                    height: frame.height,
                                    quality: frame.quality,
                                    frame_number: rate_control.lock().frame_seq(),
                                    timestamp: Utc::now(),
                                    is_keyframe,
                                    monitor_id,
                                    encode_time_us: encode_time,
                                },
                            )
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "Encode failed");
                            continue;
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Capture failed");
                    continue;
                }
            }
        };

        // Run plugin hooks on the encoded frame
        state.plugins.on_frame_encoded(&mut payload, &mut semantics);

        if payload.is_empty() {
            continue;
        }

        let encrypted = match e2e.encrypt_frame(&payload) {
            Ok(d) => d,
            Err(err) => {
                tracing::error!(error = %err, "E2E encrypt failed — skipping frame");
                continue;
            }
        };

        let sem_json = serde_json::to_vec(&semantics)?;

        if send
            .write_all(&(sem_json.len() as u32).to_be_bytes())
            .await
            .is_err()
        {
            break;
        }
        if send.write_all(&sem_json).await.is_err() {
            break;
        }
        if send
            .write_all(&(encrypted.len() as u32).to_be_bytes())
            .await
            .is_err()
        {
            break;
        }
        if send.write_all(&encrypted).await.is_err() {
            break;
        }

        state.increment_frames(&addr);

        // Record frame to disk
        {
            let mut recorder = state.recorder.lock();
            recorder.record_frame(&payload, &semantics);
        }

        // Frame-level audit
        state.audit.log(
            AuditEvent::FrameSent {
                frame_number: semantics.frame_number,
                size_bytes: payload.len() as u64,
                quality: semantics.quality,
                encode_time_us: semantics.encode_time_us,
                encrypted: e2e.is_active(),
            },
            &addr.to_string(),
            &session_id,
            &format!(
                "Frame #{}: {:.2} KB, quality {}",
                semantics.frame_number,
                payload.len() as f64 / 1024.0,
                semantics.quality
            ),
        );

        let interval_ms = (1000.0 / decision.fps as f32) as u64;
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
    }

    // Stop recording when session ends
    {
        let mut recorder = state.recorder.lock();
        if let Some(record) = recorder.stop_session() {
            tracing::info!(session = %record.session_id, "Recording session ended");
        }
    }

    // Notify plugins that session has ended
    state.plugins.on_session_end(&session_id);

    state.audit.log(
        AuditEvent::SessionEnded,
        &addr.to_string(),
        &session_id,
        "Media stream closed",
    );

    Ok(())
}

async fn handle_audio_stream(
    send: &mut SendStream,
    state: Arc<ServerState>,
    addr: SocketAddr,
) -> Result<()> {
    if !state.is_paired(&addr) {
        return Ok(());
    }

    let session_id = state.session_id(&addr);
    tracing::info!(addr = %addr, session = %session_id, "Audio stream started");

    // Run AudioCapturer on a dedicated thread because cpal::Stream is !Send.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::audio::AudioFrame>(32);
    std::thread::spawn(move || {
        let mut capturer = crate::audio::AudioCapturer::new(48000, 2);
        loop {
            match capturer.capture() {
                Ok(frame) => {
                    if tx.blocking_send(frame).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Audio capture failed");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    });

    while let Some(frame) = rx.recv().await {
        let data = serde_json::to_vec(&frame)?;
        if send
            .write_all(&(data.len() as u32).to_be_bytes())
            .await
            .is_err()
        {
            break;
        }
        if send.write_all(&data).await.is_err() {
            break;
        }
    }

    tracing::info!(addr = %addr, "Audio stream ended");
    Ok(())
}

async fn handle_debug_stream(
    send: &mut SendStream,
    recv: &mut RecvStream,
    state: Arc<ServerState>,
    addr: SocketAddr,
) -> Result<()> {
    if !state.is_paired(&addr) {
        tracing::warn!(addr = %addr, "Unpaired debug stream rejected");
        return Ok(());
    }

    tracing::info!(addr = %addr, "Debug tunnel established");

    let (to_client_tx, mut to_client_rx) = tokio::sync::mpsc::channel::<crate::debug_tunnel::CdpTunnelMessage>(256);
    let (from_client_tx, from_client_rx) = tokio::sync::mpsc::channel::<crate::debug_tunnel::CdpTunnelMessage>(256);

    let mut tunnel = crate::debug_tunnel::DebugTunnelServer::new(to_client_tx.clone(), from_client_rx);

    // Try to discover WebView2
    match tunnel.discover_webview2().await {
        Ok(()) => {
            tracing::info!(url = ?tunnel.cdp_url(), "WebView2 discovered for debug tunnel");
        }
        Err(e) => {
            tracing::warn!(error = %e, "No WebView2 found — debug tunnel will echo commands");
            tunnel.cdp_url = Some("http://127.0.0.1:0".to_string());
        }
    }

    // Spawn tunnel runner
    tokio::spawn(async move {
        if let Err(e) = tunnel.run().await {
            tracing::error!(error = %e, "Debug tunnel error");
        }
    });

    // Forward messages between QUIC stream and tunnel
    let mut len_buf = [0u8; 4];
    loop {
        tokio::select! {
            // Read from QUIC recv -> forward to tunnel
            result = recv.read_exact(&mut len_buf) => {
                match result {
                    Ok(()) => {
                        let msg_len = u32::from_be_bytes(len_buf) as usize;
                        if msg_len > 1024 * 1024 { break; }
                        let mut msg_buf = vec![0u8; msg_len];
                        if recv.read_exact(&mut msg_buf).await.is_err() { break; }
                        if let Ok(msg) = serde_json::from_slice::<crate::debug_tunnel::CdpTunnelMessage>(&msg_buf) {
                            if from_client_tx.send(msg).await.is_err() { break; }
                        }
                    }
                    Err(_) => break,
                }
            }
            // Read from tunnel -> forward to QUIC send
            msg = to_client_rx.recv() => {
                if let Some(msg) = msg {
                    if let Ok(data) = serde_json::to_vec(&msg) {
                        let len = (data.len() as u32).to_be_bytes();
                        if send.write_all(&len).await.is_err() { break; }
                        if send.write_all(&data).await.is_err() { break; }
                    }
                } else {
                    break;
                }
            }
        }
    }

    tracing::info!(addr = %addr, "Debug tunnel closed");
    Ok(())
}

async fn handle_intent_stream(
    send: &mut SendStream,
    recv: &mut RecvStream,
    state: Arc<ServerState>,
    addr: SocketAddr,
) -> Result<()> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let payload_len = u32::from_be_bytes(len_buf) as usize;
    if payload_len > 1024 * 1024 {
        return Err(anyhow::anyhow!("Intent payload too large"));
    }

    let mut data = vec![0u8; payload_len];
    recv.read_exact(&mut data).await?;

    let message: IntentMessage = match serde_json::from_slice(&data) {
        Ok(msg) => msg,
        Err(_) => IntentMessage::TextIntent(String::from_utf8_lossy(&data).to_string()),
    };

    let response = match message {
        IntentMessage::Pairing(handshake) => {
            let result = state.pair_client(
                &addr,
                &handshake.pairing_code,
                &handshake.client_name,
                &handshake.client_e2e_public,
                &handshake.pake_encrypted_key,
            );
            IntentResponse::PairingResult(result)
        }
        IntentMessage::Input(event) => {
            state.audit.log(
                AuditEvent::InputEvent {
                    action: format!("{:?}", event.action),
                },
                &addr.to_string(),
                &state.session_id(&addr),
                "Stream input",
            );
            if state.can_control(&addr) {
                match InputInjector::new() {
                    Ok(mut injector) => {
                        if let Err(err) = injector.apply(&event) {
                            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
                            IntentResponse::Error(ErrorMessage {
                                code: 1001,
                                message: format!("Input injection failed: {}", err),
                            })
                        } else {
                            IntentResponse::Ok
                        }
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "Failed to initialize input injector");
                        IntentResponse::Error(ErrorMessage {
                            code: 1003,
                            message: format!("Input system unavailable: {}", err),
                        })
                    }
                }
            } else {
                IntentResponse::Error(ErrorMessage {
                    code: 1002,
                    message: "Not authorized".to_string(),
                })
            }
        }
        IntentMessage::Heartbeat(hb) => IntentResponse::HeartbeatAck(hb),
        IntentMessage::SelectMonitor(monitor_id) => {
            let monitors = state.available_monitors();
            if monitors.iter().any(|m| m.id == monitor_id) {
                *state.selected_monitor.write() = monitor_id;
                tracing::info!(addr = %addr, monitor_id = monitor_id, "Monitor selected");
                IntentResponse::Ok
            } else {
                IntentResponse::Error(ErrorMessage {
                    code: 1004,
                    message: format!(
                        "Monitor {} not found. Available: {:?}",
                        monitor_id,
                        monitors.iter().map(|m| m.id).collect::<Vec<_>>()
                    ),
                })
            }
        }
        IntentMessage::ListMonitors => {
            IntentResponse::Monitors(state.available_monitors().to_vec())
        }
        IntentMessage::SetQuality(_) | IntentMessage::SetFps(_) => IntentResponse::Ok,
        IntentMessage::Clipboard(d) => {
            tracing::info!(addr = %addr, len = d.data.len(), "Clipboard received");
            *state.last_clipboard.write() = Some(d.clone());
            let _ = state.clipboard_update_tx.send(d);
            IntentResponse::Ok
        }
        IntentMessage::FileRequest(req) => {
            tracing::info!(addr = %addr, file = %req.filename, size = req.file_size, direction = ?req.direction, "File transfer request");
            match req.direction {
                TransferDirection::Download => {
                    let path = std::path::Path::new(&req.filename);
                    if path.exists() {
                        IntentResponse::FileResponse(req)
                    } else {
                        IntentResponse::Error(ErrorMessage {
                            code: 2001,
                            message: format!("File not found: {}", req.filename),
                        })
                    }
                }
                TransferDirection::Upload => IntentResponse::FileResponse(req),
            }
        }
        IntentMessage::FileChunk(chunk) => {
            tracing::debug!(addr = %addr, transfer = %chunk.transfer_id, offset = chunk.offset, len = chunk.data.len(), "File chunk received");

            {
                let mut transfers = state.partial_transfers.write();
                let transfer = transfers
                    .entry(chunk.transfer_id.clone())
                    .or_insert_with(|| PartialTransfer {
                        filename: String::new(),
                        total_size: 0,
                        received_bytes: 0,
                        chunks_received: Vec::new(),
                        created_at: std::time::Instant::now(),
                    });
                transfer.chunks_received.push(chunk.offset);
                transfer.received_bytes += chunk.data.len() as u64;

                if chunk.is_last {
                    tracing::info!(
                        transfer = %chunk.transfer_id,
                        bytes = transfer.received_bytes,
                        "File transfer complete"
                    );
                    transfers.remove(&chunk.transfer_id);
                }
            }

            let output_dir = std::env::temp_dir().join("continuum-transfers");
            let _ = std::fs::create_dir_all(&output_dir);
            let file_path = output_dir.join(&chunk.transfer_id);

            use std::io::{Seek, SeekFrom, Write};
            match std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&file_path)
            {
                Ok(mut file) => {
                    let _ = file.seek(SeekFrom::Start(chunk.offset));
                    let _ = file.write_all(&chunk.data);
                    if chunk.is_last {
                        tracing::info!(file = %file_path.display(), "File transfer complete");
                    }
                    IntentResponse::Ok
                }
                Err(e) => IntentResponse::Error(ErrorMessage {
                    code: 2002,
                    message: format!("Failed to write: {}", e),
                }),
            }
        }
        IntentMessage::TextIntent(text) => {
            tracing::info!(addr = %addr, intent = %text, "Text intent");
            IntentResponse::Ok
        }
        IntentMessage::Resume(req) => {
            let resume_data = state.resume_tokens.read().get(&req.token).cloned();
            match resume_data {
                Some(session) if session.created_at.elapsed() < Duration::from_secs(3600) => {
                    let mut clients = state.clients.write();
                    if let Some(client) = clients.get_mut(&addr) {
                        client.paired = session.paired;
                        client.permissions = session.permissions.clone();
                        client.shared_secret = session.shared_secret;
                        tracing::info!(addr = %addr, session = %session.session_id, "Session resumed");
                        IntentResponse::PairingResult(PairingResponse {
                            accepted: true,
                            message: "Session resumed".to_string(),
                            session_token: Some(req.token.clone()),
                            permissions: session.permissions,
                            server_e2e_public: Vec::new(),
                            pake_encrypted_key: Vec::new(),
                            resume_token: None,
                            sas_words: Vec::new(),
                        })
                    } else {
                        IntentResponse::Error(ErrorMessage {
                            code: 1005,
                            message: "Client not registered".to_string(),
                        })
                    }
                }
                Some(_) => IntentResponse::Error(ErrorMessage {
                    code: 1006,
                    message: "Resume token expired".to_string(),
                }),
                None => IntentResponse::Error(ErrorMessage {
                    code: 1007,
                    message: "Invalid resume token".to_string(),
                }),
            }
        }
    };

    let resp_json = serde_json::to_vec(&response)?;
    send.write_all(&(resp_json.len() as u32).to_be_bytes())
        .await?;
    send.write_all(&resp_json).await?;
    let _ = send.finish();
    Ok(())
}

async fn handle_clipboard_stream(
    _send: &mut SendStream,
    recv: &mut RecvStream,
    state: Arc<ServerState>,
    addr: SocketAddr,
) -> Result<()> {
    if !state.is_paired(&addr) {
        return Ok(());
    }

    loop {
        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 16 * 1024 * 1024 {
            break;
        }
        let mut data = vec![0u8; len];
        if recv.read_exact(&mut data).await.is_err() {
            break;
        }
        if let Ok(clipboard) = serde_json::from_slice::<ClipboardData>(&data) {
            *state.last_clipboard.write() = Some(clipboard.clone());
            let _ = state.clipboard_update_tx.send(clipboard);
        }
    }

    Ok(())
}

async fn handle_file_transfer_stream(
    send: &mut SendStream,
    recv: &mut RecvStream,
    state: Arc<ServerState>,
    addr: SocketAddr,
) -> Result<()> {
    if !state.is_paired(&addr) {
        return Ok(());
    }

    loop {
        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let payload_len = u32::from_be_bytes(len_buf) as usize;
        if payload_len > 1024 * 1024 {
            break;
        }
        let mut data = vec![0u8; payload_len];
        if recv.read_exact(&mut data).await.is_err() {
            break;
        }
        let message: IntentMessage = match serde_json::from_slice(&data) {
            Ok(msg) => msg,
            Err(_) => IntentMessage::TextIntent(String::from_utf8_lossy(&data).to_string()),
        };

        let response = match message {
            IntentMessage::FileRequest(req) => {
                tracing::info!(addr = %addr, file = %req.filename, size = req.file_size, direction = ?req.direction, "File transfer request");
                match req.direction {
                    TransferDirection::Download => {
                        let path = std::path::Path::new(&req.filename);
                        if path.exists() {
                            IntentResponse::FileResponse(req)
                        } else {
                            IntentResponse::Error(ErrorMessage {
                                code: 2001,
                                message: format!("File not found: {}", req.filename),
                            })
                        }
                    }
                    TransferDirection::Upload => IntentResponse::FileResponse(req),
                }
            }
            IntentMessage::FileChunk(chunk) => {
                tracing::debug!(addr = %addr, transfer = %chunk.transfer_id, offset = chunk.offset, len = chunk.data.len(), "File chunk received");
                let output_dir = std::env::temp_dir().join("continuum-transfers");
                let _ = std::fs::create_dir_all(&output_dir);
                let file_path = output_dir.join(&chunk.transfer_id);

                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&file_path)
                {
                    use std::io::{Seek, SeekFrom, Write};
                    let _ = file.seek(SeekFrom::Start(chunk.offset));
                    let _ = file.write_all(&chunk.data);
                    if chunk.is_last {
                        tracing::info!(file = %file_path.display(), "File transfer complete");
                    }
                }
                IntentResponse::Ok
            }
            _ => IntentResponse::Ok,
        };

        let resp_json = serde_json::to_vec(&response)?;
        send.write_all(&(resp_json.len() as u32).to_be_bytes())
            .await?;
        send.write_all(&resp_json).await?;
    }

    Ok(())
}
