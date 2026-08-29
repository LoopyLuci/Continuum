use anyhow::Result;
use clap::Parser;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "relay-server", version, about = "Continuum QUIC relay server")]
struct RelayArgs {
    #[arg(short, long, default_value = "0.0.0.0:4434")]
    listen: SocketAddr,

    #[arg(long, help = "Log level")]
    log_level: Option<String>,

    #[arg(long, default_value = "continuum-relay", help = "Shared relay secret for authentication")]
    secret: String,
}

#[derive(Debug, Clone)]
struct RegisteredSession {
    #[allow(dead_code)]
    addr: SocketAddr,
    connection: Connection,
    connected_at: std::time::Instant,
}

struct RelayState {
    sessions: RwLock<HashMap<String, RegisteredSession>>,
    secret: String,
}

impl RelayState {
    fn new(secret: String) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            secret,
        }
    }

    async fn register_session(&self, session_id: String, conn: Connection) -> Result<()> {
        let addr = conn.remote_address();
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session_id) {
            anyhow::bail!("Session already registered: {}", session_id);
        }
        sessions.insert(
            session_id.clone(),
            RegisteredSession {
                addr,
                connection: conn,
                connected_at: std::time::Instant::now(),
            },
        );
        tracing::info!(session = %session_id, addr = %addr, count = sessions.len(), "Session registered");
        Ok(())
    }

    async fn get_or_wait_session(&self, session_id: String) -> Option<RegisteredSession> {
        loop {
            {
                let sessions = self.sessions.read().await;
                if let Some(session) = sessions.get(&session_id) {
                    return Some(session.clone());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn cleanup_expired(&self) {
        let mut sessions = self.sessions.write().await;
        let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(3600);
        sessions.retain(|_, session| session.connected_at > cutoff);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = RelayArgs::parse();
    let level = args.log_level.as_deref().unwrap_or("info");
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)),
        )
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert = rcgen::generate_simple_self_signed(vec![
        "localhost".into(),
        "relay.continuum.local".into(),
    ])?;
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
    let cert_der: rustls::pki_types::CertificateDer<'static> = cert.cert.der().as_ref().to_vec().into();

    let mut rustls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key.into())?;
    rustls_config.alpn_protocols = vec![b"continuum-relay-1".to_vec()];

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_bidi_streams(256u32.into());
    transport_config.max_concurrent_uni_streams(128u32.into());
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
    transport_config.max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into().unwrap()));

    let server_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_config)?;
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
    server_config.transport_config(Arc::new(transport_config));

    let endpoint = Endpoint::server(server_config, args.listen)?;
    let state = Arc::new(RelayState::new(args.secret));

    tracing::info!(addr = %args.listen, "Continuum relay server listening");

    tokio::spawn({
        let state = state.clone();
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                state.cleanup_expired().await;
            }
        }
    });

    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let addr = conn.remote_address();
                    tracing::info!(addr = %addr, "Relay connection");
                    if let Err(err) = handle_relay_connection(conn, state).await {
                        tracing::error!(addr = %addr, error = %err, "Relay error");
                    }
                }
                Err(err) => tracing::error!(error = %err, "Failed to accept relay connection"),
            }
        });
    }

    Ok(())
}

async fn handle_relay_connection(conn: Connection, state: Arc<RelayState>) -> Result<()> {
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let state = state.clone();
                let conn = conn.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_relay_stream(send, recv, state, conn).await {
                        tracing::debug!(error = %err, "Stream error");
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
            Err(err) => {
                tracing::error!(error = %err, "Connection error");
                break;
            }
        }
    }
    Ok(())
}

async fn handle_relay_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    state: Arc<RelayState>,
    conn: Connection,
) -> Result<()> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        return Err(anyhow::anyhow!("Relay message too large: {}", len));
    }

    let mut data = vec![0u8; len];
    recv.read_exact(&mut data).await?;

    let message: serde_json::Value = serde_json::from_slice(&data)?;
    let msg_type = message.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "register" => {
            let session_id = message.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            let secret = message.get("secret").and_then(|v| v.as_str()).unwrap_or("");
            if secret != state.secret {
                send_error(&mut send, "Invalid relay secret").await?;
                return Ok(());
            }
            if session_id.is_empty() {
                send_error(&mut send, "Missing session_id").await?;
                return Ok(());
            }

            state.register_session(session_id.into(), conn).await?;
            let response = b"{\"type\":\"register_ack\"}";
            send.write_all(&(response.len() as u32).to_be_bytes()).await?;
            send.write_all(response).await?;
            let _ = send.finish();
        }
        "connect" => {
            let session_id = message.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            let secret = message.get("secret").and_then(|v| v.as_str()).unwrap_or("");
            if secret != state.secret {
                send_error(&mut send, "Invalid relay secret").await?;
                return Ok(());
            }
            if session_id.is_empty() {
                send_error(&mut send, "Missing session_id").await?;
                return Ok(());
            }

            let Some(host) = state.get_or_wait_session(session_id.into()).await else {
                send_error(&mut send, "Session not found").await?;
                return Ok(());
            };

            let response = b"{\"type\":\"connect_ack\"}";
            send.write_all(&(response.len() as u32).to_be_bytes()).await?;
            send.write_all(response).await?;

            let Ok((mut host_send, mut host_recv)) = host.connection.open_bi().await else {
                send_error(&mut send, "Failed to open host stream").await?;
                return Ok(());
            };

            let ready = b"{\"type\":\"relay_ready\"}";
            let _ = host_send
                .write_all(&(ready.len() as u32).to_be_bytes())
                .await;
            let _ = host_send.write_all(ready).await;

            let _ = tokio::join!(
                async {
                    loop {
                        let mut len_buf = [0u8; 4];
                        if recv.read_exact(&mut len_buf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        if len > 64 * 1024 * 1024 {
                            break;
                        }
                        let mut buf = vec![0u8; len];
                        if recv.read_exact(&mut buf).await.is_err() {
                            break;
                        }
                        if host_send.write_all(&(len as u32).to_be_bytes()).await.is_err() {
                            break;
                        }
                        if host_send.write_all(&buf).await.is_err() {
                            break;
                        }
                    }
                },
                async {
                    loop {
                        let mut len_buf = [0u8; 4];
                        if host_recv.read_exact(&mut len_buf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        if len > 64 * 1024 * 1024 {
                            break;
                        }
                        let mut buf = vec![0u8; len];
                        if host_recv.read_exact(&mut buf).await.is_err() {
                            break;
                        }
                        if send.write_all(&(len as u32).to_be_bytes()).await.is_err() {
                            break;
                        }
                        if send.write_all(&buf).await.is_err() {
                            break;
                        }
                    }
                }
            );
        }
        _ => send_error(&mut send, "Unknown message type").await?,
    }

    Ok(())
}

async fn send_error(send: &mut SendStream, message: &str) -> Result<()> {
    let payload = serde_json::json!({
        "type": "error",
        "message": message,
    });
    let bytes = payload.to_string().into_bytes();
    send.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    let _ = send.finish();
    Ok(())
}
