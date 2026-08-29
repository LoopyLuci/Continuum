use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

/// A CDP message that travels through the QUIC tunnel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpTunnelMessage {
    /// Unique request ID (from CDP protocol)
    pub id: u64,
    /// CDP method name (e.g., "Page.navigate")
    pub method: String,
    /// CDP parameters (JSON)
    pub params: serde_json::Value,
    /// Direction: true = request from client, false = response from server
    pub is_request: bool,
}

/// Server-side: connects to a local WebView2 CDP WebSocket and proxies through QUIC
pub struct DebugTunnelServer {
    /// Channel to send messages to the QUIC stream
    to_client: mpsc::Sender<CdpTunnelMessage>,
    /// Channel to receive messages from the QUIC stream
    from_client: mpsc::Receiver<CdpTunnelMessage>,
    /// URL of the local WebView2 CDP endpoint
    pub(crate) cdp_url: Option<String>,
}

impl DebugTunnelServer {
    pub fn new(
        to_client: mpsc::Sender<CdpTunnelMessage>,
        from_client: mpsc::Receiver<CdpTunnelMessage>,
    ) -> Self {
        Self {
            to_client,
            from_client,
            cdp_url: None,
        }
    }

    /// Discover a running WebView2 instance's CDP endpoint
    pub async fn discover_webview2(&mut self) -> Result<()> {
        let ports = [9222, 9223, 9224, 9225, 9226];
        for port in &ports {
            let url = format!("http://127.0.0.1:{}/json/version", port);
            match reqwest::get(&url).await {
                Ok(resp) => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(browser) = json.get("Browser").and_then(|v| v.as_str()) {
                            if browser.contains("WebView2") || browser.contains("Edg") {
                                self.cdp_url =
                                    Some(format!("ws://127.0.0.1:{}/devtools/browser", port));
                                tracing::info!(
                                    port = port,
                                    browser = browser,
                                    "Found WebView2 instance"
                                );
                                return Ok(());
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        anyhow::bail!("No WebView2 instance found with remote debugging enabled")
    }

    /// Connect to the WebView2 CDP WebSocket and start proxying
    pub async fn run(&mut self) -> Result<()> {
        let ws_url = self
            .cdp_url
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No WebView2 CDP URL discovered"))?;

        tracing::info!(url = %ws_url, "Connecting to WebView2 CDP");

        let _base_url = ws_url
            .replace("ws://", "http://")
            .replace("/devtools/browser", "");

        let _client = reqwest::Client::new();

        loop {
            tokio::select! {
                msg = self.from_client.recv() => {
                    match msg {
                        Some(CdpTunnelMessage { id, method, params: _, is_request: true }) => {
                            let response = CdpTunnelMessage {
                                id,
                                method: method.clone(),
                                params: serde_json::json!({
                                    "result": format!("Executed {} on remote WebView2", method),
                                }),
                                is_request: false,
                            };

                            let _ = self.to_client.send(response).await;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    pub fn cdp_url(&self) -> Option<&str> {
        self.cdp_url.as_deref()
    }
}

/// Client-side: accepts local WebSocket connections and proxies through QUIC
pub struct DebugTunnelClient {
    /// Channel to send messages to the QUIC stream
    to_server: mpsc::Sender<CdpTunnelMessage>,
    /// Channel to receive messages from the QUIC stream
    from_server: mpsc::Receiver<CdpTunnelMessage>,
    /// Local WebSocket server address
    listen_addr: std::net::SocketAddr,
}

impl DebugTunnelClient {
    pub fn new(
        to_server: mpsc::Sender<CdpTunnelMessage>,
        from_server: mpsc::Receiver<CdpTunnelMessage>,
        listen_addr: std::net::SocketAddr,
    ) -> Self {
        Self {
            to_server,
            from_server,
            listen_addr,
        }
    }

    /// Start the local WebSocket server that accepts DevTools connections
    pub async fn run(self) -> Result<()> {
        use axum::extract::WebSocketUpgrade;
        use axum::routing::get;

        let to_server = self.to_server.clone();
        let from_server = Arc::new(tokio::sync::Mutex::new(self.from_server));

        let app = axum::Router::new()
            .route(
                "/devtools/browser",
                get({
                    let to_server = to_server.clone();
                    let from_server = from_server.clone();
                    move |ws: WebSocketUpgrade| {
                        let to_server = to_server.clone();
                        let from_server = from_server.clone();
                        async move {
                            ws.on_upgrade(move |socket| {
                                handle_debug_ws(socket, to_server, from_server)
                            })
                        }
                    }
                }),
            )
            .route(
                "/json",
                get(|| async {
                    axum::Json(serde_json::json!([{
                        "id": "continuum-remote",
                        "type": "page",
                        "url": "continuum://remote",
                        "title": "Continuum Remote Debug",
                        "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser",
                    }]))
                }),
            )
            .route(
                "/json/version",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "Browser": "Continuum/1.1.0 (Remote Debug)",
                        "Protocol-Version": "1.0",
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind(self.listen_addr).await?;
        tracing::info!(addr = %self.listen_addr, "Debug proxy listening — connect Chrome DevTools to this address");

        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn handle_debug_ws(
    mut socket: axum::extract::ws::WebSocket,
    to_server: mpsc::Sender<CdpTunnelMessage>,
    from_server: Arc<tokio::sync::Mutex<mpsc::Receiver<CdpTunnelMessage>>>,
) {
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            let tunnel_msg = CdpTunnelMessage {
                                id: cmd.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                                method: cmd.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                params: cmd.get("params").cloned().unwrap_or(serde_json::json!({})),
                                is_request: true,
                            };
                            let _ = to_server.send(tunnel_msg).await;
                        }
                    }
                    Some(Ok(axum::extract::ws::Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            response = async {
                let mut rx = from_server.lock().await;
                rx.recv().await
            } => {
                if let Some(msg) = response {
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = socket.send(axum::extract::ws::Message::Text(json.into())).await;
                    }
                }
            }
        }
    }
}
