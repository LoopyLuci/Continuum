// CDP-compatible debug protocol for WebView2-style agent inspection
// =================================================================
// Provides WebSocket endpoint for real-time state updates and
// HTTP endpoints for imperative control, compatible with CDP agents.
// =================================================================

use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct DebugState {
    pub state: Arc<RwLock<AppStateSnapshot>>,
    pub event_tx: broadcast::Sender<DebugEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppStateSnapshot {
    pub app_mode: String,
    pub wizard_step: String,
    pub connection_status: String,
    pub server_address: Option<String>,
    pub machine_id: String,
    pub pairing_code: String,
    pub fps: f32,
    pub frame_count: u64,
    pub bytes_received: u64,
    pub uptime_secs: u64,
    pub elements: Vec<UiElement>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiElement {
    pub node_id: String,
    pub tag: String,
    pub bounds: ElementBounds,
    pub visible: bool,
    pub focused: bool,
    pub text: Option<String>,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugEvent {
    StateUpdate { state: AppStateSnapshot },
    FrameReceived { size: u64 },
    InputEvent { event_type: String },
    Error { message: String },
}

#[derive(Deserialize)]
pub(crate) struct CdpCommand {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Serialize)]
pub(crate) struct CdpResponse {
    pub id: u64,
    pub result: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn start_cdp_server(state: DebugState, addr: SocketAddr) {
    let app = Router::new()
        .route("/json", get(handle_json_list))
        .route("/json/version", get(handle_version))
        .route("/json/protocol", get(handle_protocol))
        .route("/devtools/browser", get(handle_websocket))
        .route("/", get(handle_index))
        .with_state(state);

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            tracing::info!(addr = %addr, "CDP debug server started");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(error = %e, "CDP server error");
            }
        }
        Err(e) => tracing::error!(error = %e, "Failed to start CDP server"),
    }
}

pub(crate) async fn handle_json_list(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.state.read();
    Json(serde_json::json!([{
        "id": "continuum",
        "type": "page",
        "url": format!("continuum://localhost:{}", s.machine_id),
        "title": "Continuum",
        "webSocketDebuggerUrl": "ws://127.0.0.1:9094/devtools/page/continuum".to_string(),
    }]))
}

pub(crate) async fn handle_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "Browser": format!("Continuum/{}", env!("CARGO_PKG_VERSION")),
        "Protocol-Version": "1.0",
        "V8-Version": "N/A",
        "WebKit-Version": "N/A",
        "User-Agent": "Continuum/0.2.0",
    }))
}

pub(crate) async fn handle_protocol() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "domains": [
            { "domain": "Page", "description": "Page lifecycle" },
            { "domain": "Runtime", "description": "JavaScript execution" },
            { "domain": "Input", "description": "Input simulation" },
            { "domain": "DOM", "description": "DOM inspection" },
        ]
    }))
}

pub(crate) async fn handle_index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "description": "Continuum DevTools Protocol endpoint",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn handle_websocket(
    State(state): State<DebugState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
}

async fn handle_ws_connection(mut socket: WebSocket, state: DebugState) {
    let mut rx = state.event_tx.subscribe();

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<CdpCommand>(&text) {
                            let response = process_cdp_command(&state, &cmd).await;
                            match serde_json::to_string(&response) {
                                Ok(json) => {
                                    let _ = socket.send(axum::extract::ws::Message::Text(json.into())).await;
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "Failed to serialize CDP response");
                                }
                            }
                        }
                    }
                    Some(Ok(axum::extract::ws::Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            Ok(evt) = rx.recv() => {
                if let Ok(text) = serde_json::to_string(&evt) {
                    let _ = socket.send(axum::extract::ws::Message::Text(text.into())).await;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

pub(crate) async fn process_cdp_command(state: &DebugState, cmd: &CdpCommand) -> CdpResponse {
    let result = match cmd.method.as_str() {
        "Page.captureScreenshot" => {
            let s = state.state.read();
            serde_json::json!({
                "data": format!("{} frames captured", s.frame_count),
            })
        }
        "Runtime.evaluate" => {
            serde_json::json!({
                "result": { "type": "string", "value": "Continuum Runtime" },
            })
        }
        "DOM.getDocument" => {
            let s = state.state.read();
            serde_json::json!({
                "root": {
                    "nodeName": "CONTINUUM",
                    "nodeId": 1,
                    "children": s.elements.iter().map(|e| {
                        serde_json::json!({
                            "nodeName": e.tag,
                            "nodeId": e.node_id,
                            "attributes": ["text", e.text.as_deref().unwrap_or("")]
                        })
                    }).collect::<Vec<_>>(),
                }
            })
        }
        "DOM.getBoxModel" => {
            let node_id = cmd
                .params
                .get("nodeId")
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            let s = state.state.read();
            let elem = s.elements.iter().find(|e| e.node_id == node_id.to_string());
            match elem {
                Some(e) => serde_json::json!({
                    "model": {
                        "content": [e.bounds.x, e.bounds.y, e.bounds.x + e.bounds.width, e.bounds.y, e.bounds.x + e.bounds.width, e.bounds.y + e.bounds.height, e.bounds.x, e.bounds.y + e.bounds.height],
                        "width": e.bounds.width,
                        "height": e.bounds.height,
                    }
                }),
                None => serde_json::json!({ "error": "Node not found" }),
            }
        }
        "Input.dispatchMouseEvent" => {
            serde_json::json!({ "handled": true })
        }
        "Input.dispatchKeyEvent" => {
            serde_json::json!({ "handled": true })
        }
        "Runtime.getProperties" => {
            serde_json::json!({ "result": {} })
        }
        "Page.enable" | "DOM.enable" | "Runtime.enable" => {
            serde_json::json!({})
        }
        _ => serde_json::json!({ "error": format!("Unknown method: {}", cmd.method) }),
    };

    CdpResponse {
        id: cmd.id,
        result,
        error: None,
    }
}
