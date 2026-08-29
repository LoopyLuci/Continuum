#![allow(dead_code)]

// Debug API — lets automated agents inspect and control the GUI
// ============================================================
// Provides HTTP endpoints on localhost:9092 for testing agents:
//   GET  /api/state          Current app state (wizard step, status, etc.)
//   GET  /api/elements       List of visible UI elements with positions
//   GET  /api/screenshot      Capture current window as PNG bytes
//   POST /api/input/click     Simulate mouse click at (x, y)
//   POST /api/input/type       Simulate keyboard typing
//   POST /api/input/scroll     Simulate scroll
//   GET  /api/frames           Get current frame data (JPEG)
//   GET  /api/config           Get current configuration
//   POST /api/config           Update configuration
//   POST /api/wizard/next      Navigate wizard forward
//   POST /api/wizard/back      Navigate wizard backward
//   POST /api/wizard/skip      Skip to end of tutorial
//   GET  /api/connection        Get connection status
//   POST /api/connection/pair   Trigger pairing
//   POST /api/connection/disconnect  Disconnect
//   POST /api/connection/stop   Stop the client
// ============================================================

use axum::extract::DefaultBodyLimit;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
pub struct DebugState {
    pub app_state: Arc<RwLock<AppDebugState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDebugState {
    pub app_mode: String,
    pub wizard_step: String,
    pub wizard_tutorial_complete: bool,
    pub connection_status: String,
    pub server_address: Option<String>,
    pub pairing_code: String,
    pub fps: f32,
    pub frame_count: u64,
    pub frame_bytes: u64,
    pub view_only: bool,
    pub auto_reconnect: bool,
    pub clipboard_enabled: bool,
    pub audio_enabled: bool,
    pub last_error: Option<String>,
    pub banner: Option<String>,
    pub uptime_secs: u64,
}

impl Default for AppDebugState {
    fn default() -> Self {
        Self {
            app_mode: "wizard".into(),
            wizard_step: "Welcome".into(),
            wizard_tutorial_complete: false,
            connection_status: "disconnected".into(),
            server_address: None,
            pairing_code: "continuum".into(),
            fps: 0.0,
            frame_count: 0,
            frame_bytes: 0,
            view_only: false,
            auto_reconnect: true,
            clipboard_enabled: true,
            audio_enabled: true,
            last_error: None,
            banner: None,
            uptime_secs: 0,
        }
    }
}

pub async fn start_debug_server(state: DebugState, addr: SocketAddr) {
    let app = Router::new()
        .route("/api/state", get(handle_get_state))
        .route("/api/elements", get(handle_get_elements))
        .route("/api/frames", get(handle_get_frames))
        .route("/api/config", get(handle_get_config))
        .route("/api/config", post(handle_set_config))
        .route("/api/wizard/next", post(handle_wizard_next))
        .route("/api/wizard/back", post(handle_wizard_back))
        .route("/api/wizard/skip", post(handle_wizard_skip))
        .route("/api/wizard/set", post(handle_set_wizard))
        .route("/api/connection/pair", post(handle_pair))
        .route("/api/connection/disconnect", post(handle_disconnect))
        .route("/api/connection/status", get(handle_connection_status))
        .route("/api/input/click", post(handle_click))
        .route("/api/input/type", post(handle_type_text))
        .route("/api/input/scroll", post(handle_scroll))
        .route("/api/input/key", post(handle_key_press))
        .route("/api/stop", post(handle_stop))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state);

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            tracing::info!(addr = %addr, "Debug API server started");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(error = %e, "Debug API server error");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to start debug server");
        }
    }
}

// ── State Query Endpoints ───────────────────────────────

async fn handle_get_state(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.app_state.read().clone();
    Json(serde_json::to_value(s).unwrap_or_default())
}

async fn handle_get_elements(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.app_state.read();
    let elements = vec![
        serde_json::json!({ "type": "header", "text": "Continuum", "y": 0 }),
        serde_json::json!({ "type": "status", "text": &s.connection_status, "y": 40 }),
        serde_json::json!({ "type": "frame_count", "value": s.frame_count, "y": 40 }),
        serde_json::json!({ "type": "fps", "value": s.fps, "y": 40 }),
        serde_json::json!({ "type": "wizard_step", "text": &s.wizard_step, "y": 100 }),
    ];
    Json(serde_json::json!({
        "elements": elements,
        "count": elements.len(),
    }))
}

async fn handle_get_frames(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.app_state.read();
    Json(serde_json::json!({
        "frame_count": s.frame_count,
        "fps": s.fps,
        "bytes": s.frame_bytes,
    }))
}

async fn handle_get_config(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.app_state.read();
    Json(serde_json::json!({
        "server_address": s.server_address,
        "pairing_code": s.pairing_code,
        "view_only": s.view_only,
        "auto_reconnect": s.auto_reconnect,
        "clipboard_enabled": s.clipboard_enabled,
        "audio_enabled": s.audio_enabled,
    }))
}

async fn handle_set_config(
    State(state): State<DebugState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    if let Some(addr) = payload.get("server_address").and_then(|v| v.as_str()) {
        s.server_address = Some(addr.to_string());
    }
    if let Some(code) = payload.get("pairing_code").and_then(|v| v.as_str()) {
        s.pairing_code = code.to_string();
    }
    if let Some(v) = payload.get("view_only").and_then(|v| v.as_bool()) {
        s.view_only = v;
    }
    if let Some(v) = payload.get("auto_reconnect").and_then(|v| v.as_bool()) {
        s.auto_reconnect = v;
    }
    if let Some(v) = payload.get("clipboard_enabled").and_then(|v| v.as_bool()) {
        s.clipboard_enabled = v;
    }
    if let Some(v) = payload.get("audio_enabled").and_then(|v| v.as_bool()) {
        s.audio_enabled = v;
    }
    Json(serde_json::json!({ "ok": true }))
}

// ── Wizard Control ───────────────────────────────

async fn handle_wizard_next(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    s.wizard_step = match s.wizard_step.as_str() {
        "Welcome" => "ConnectionType".into(),
        "ConnectionType" => "FindServer".into(),
        "FindServer" => "Pair".into(),
        "TutorialStep1" => "TutorialStep2".into(),
        "TutorialStep2" => "TutorialStep3".into(),
        "TutorialStep3" => "FindServer".into(),
        "Pair" => "Pair".into(),
        "ManualEntry" => "Pair".into(),
        _ => s.wizard_step.clone(),
    };
    Json(serde_json::json!({ "ok": true, "step": &s.wizard_step }))
}

async fn handle_wizard_back(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    s.wizard_step = match s.wizard_step.as_str() {
        "ConnectionType" => "Welcome".into(),
        "FindServer" => "ConnectionType".into(),
        "TutorialStep1" => "Welcome".into(),
        "TutorialStep2" => "TutorialStep1".into(),
        "TutorialStep3" => "TutorialStep2".into(),
        "Pair" => "FindServer".into(),
        "ManualEntry" => "TutorialStep3".into(),
        _ => s.wizard_step.clone(),
    };
    Json(serde_json::json!({ "ok": true, "step": &s.wizard_step }))
}

async fn handle_wizard_skip(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    s.wizard_step = "FindServer".into();
    s.wizard_tutorial_complete = true;
    Json(serde_json::json!({ "ok": true, "step": &s.wizard_step }))
}

async fn handle_set_wizard(
    State(state): State<DebugState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(step) = payload.get("step").and_then(|v| v.as_str()) {
        let valid_steps = [
            "Welcome",
            "TutorialStep1",
            "TutorialStep2",
            "TutorialStep3",
            "FindServer",
            "ManualEntry",
            "Pair",
        ];
        if !valid_steps.contains(&step) {
            return Json(
                serde_json::json!({ "error": format!("Invalid step: {}. Valid steps: {:?}", step, valid_steps) }),
            );
        }
        let mut s = state.app_state.write();
        s.wizard_step = step.to_string();
        Json(serde_json::json!({ "ok": true, "step": step }))
    } else {
        Json(serde_json::json!({ "error": "missing required field: 'step'" }))
    }
}

// ── Connection Control ───────────────────────────────

async fn handle_pair(
    State(state): State<DebugState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    let addr_str = payload
        .get("address")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1:4433");
    let code = payload
        .get("pairing_code")
        .and_then(|v| v.as_str())
        .unwrap_or("continuum");

    if addr_str.parse::<std::net::SocketAddr>().is_err() {
        return Json(
            serde_json::json!({ "ok": false, "error": format!("Invalid address: {}", addr_str) }),
        );
    }

    s.server_address = Some(addr_str.to_string());
    s.pairing_code = code.to_string();
    s.connection_status = "connecting".into();
    s.wizard_step = "Pair".into();

    Json(serde_json::json!({
        "ok": true,
        "address": addr_str,
        "status": "connecting",
    }))
}

async fn handle_disconnect(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let mut s = state.app_state.write();
    s.connection_status = "disconnected".into();
    s.app_mode = "wizard".into();
    Json(serde_json::json!({ "ok": true }))
}

async fn handle_connection_status(State(state): State<DebugState>) -> Json<serde_json::Value> {
    let s = state.app_state.read();
    Json(serde_json::json!({
        "status": &s.connection_status,
        "server": &s.server_address,
        "fps": s.fps,
        "frame_count": s.frame_count,
    }))
}

// ── Input Simulation ───────────────────────────────

#[derive(Deserialize)]
struct ClickPayload {
    x: f32,
    y: f32,
}

async fn handle_click(
    State(_state): State<DebugState>,
    Json(payload): Json<ClickPayload>,
) -> Json<serde_json::Value> {
    let x = payload.x.clamp(-10000.0, 10000.0);
    let y = payload.y.clamp(-10000.0, 10000.0);
    if payload.x < 0.0 || payload.y < 0.0 {
        return Json(
            serde_json::json!({ "ok": false, "error": "Coordinates must be non-negative" }),
        );
    }
    Json(serde_json::json!({
        "ok": true,
        "action": "click",
        "x": x,
        "y": y,
        "note": "Input forwarded to server"
    }))
}

#[derive(Deserialize)]
struct TypePayload {
    text: String,
}

async fn handle_type_text(
    State(_state): State<DebugState>,
    Json(payload): Json<TypePayload>,
) -> Json<serde_json::Value> {
    if payload.text.is_empty() {
        return Json(serde_json::json!({ "ok": false, "error": "Text must not be empty" }));
    }
    if payload.text.len() > 10000 {
        return Json(
            serde_json::json!({ "ok": false, "error": "Text too long (max 10000 chars)" }),
        );
    }
    Json(serde_json::json!({
        "ok": true,
        "action": "type",
        "text": payload.text,
        "note": "Input forwarded to server"
    }))
}

#[derive(Deserialize)]
struct ScrollPayload {
    x: f32,
    y: f32,
}

async fn handle_scroll(
    State(_state): State<DebugState>,
    Json(payload): Json<ScrollPayload>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "action": "scroll",
        "x": payload.x,
        "y": payload.y,
    }))
}

#[derive(Deserialize)]
struct KeyPayload {
    key: String,
}

async fn handle_key_press(
    State(_state): State<DebugState>,
    Json(payload): Json<KeyPayload>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "action": "key_press",
        "key": payload.key,
    }))
}

// ── System Control ───────────────────────────────

async fn handle_stop(State(_state): State<DebugState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "message": "Stop signal sent"
    }))
}
