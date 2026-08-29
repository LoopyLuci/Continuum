use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct HealthState {
    pub start_time: Instant,
    pub connections: Arc<std::sync::atomic::AtomicI64>,
    pub last_frame_time: Arc<RwLock<Option<Instant>>>,
    pub frames_encoded: Arc<std::sync::atomic::AtomicU64>,
    pub frames_decoded: Arc<std::sync::atomic::AtomicU64>,
    pub errors: Arc<RwLock<Vec<String>>>,
    pub extra: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            connections: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            last_frame_time: Arc::new(RwLock::new(None)),
            frames_encoded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            frames_decoded: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            errors: Arc::new(RwLock::new(Vec::new())),
            extra: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    uptime_secs: u64,
    version: &'static str,
    active_connections: i64,
    frames_encoded: u64,
    frames_decoded: u64,
    last_frame_ago_secs: Option<f64>,
    recent_errors: Vec<String>,
    extra: HashMap<String, String>,
}

pub struct HealthServer {
    pub state: HealthState,
}

impl Default for HealthServer {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthServer {
    pub fn new() -> Self {
        Self {
            state: HealthState::new(),
        }
    }

    pub fn app(&self) -> Router {
        let state = self.state.clone();
        Router::new()
            .route("/health", get(handle_health))
            .route("/health/prometheus", get(handle_prometheus))
            .route("/health/ready", get(handle_readyness))
            .with_state(state)
    }

    pub async fn serve(&self, addr: &str) {
        let app = self.app();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        tracing::info!(addr = %addr, "Health server started");
        axum::serve(listener, app).await.unwrap();
    }
}

async fn handle_health(State(state): State<HealthState>) -> Json<HealthResponse> {
    let last_frame = *state.last_frame_time.read();
    let last_frame_ago = last_frame.map(|t| t.elapsed().as_secs_f64());

    let errors = state.errors.read().clone();
    let extra = state.extra.read().clone();

    Json(HealthResponse {
        status: "ok",
        uptime_secs: state.start_time.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION"),
        active_connections: state.connections.load(std::sync::atomic::Ordering::Relaxed),
        frames_encoded: state
            .frames_encoded
            .load(std::sync::atomic::Ordering::Relaxed),
        frames_decoded: state
            .frames_decoded
            .load(std::sync::atomic::Ordering::Relaxed),
        last_frame_ago_secs: last_frame_ago,
        recent_errors: errors,
        extra,
    })
}

async fn handle_readyness() -> Response {
    (StatusCode::OK, "ready").into_response()
}

async fn handle_prometheus(State(state): State<HealthState>) -> String {
    let uptime = state.start_time.elapsed().as_secs_f64();
    format!(
        "# HELP continuum_up Uptime in seconds\n\
         # TYPE continuum_up gauge\n\
         continuum_up {uptime}\n\
         # HELP continuum_connections Active connections\n\
         # TYPE continuum_connections gauge\n\
         continuum_connections {conn}\n\
         # HELP continuum_frames_encoded Total frames encoded\n\
         # TYPE continuum_frames_encoded counter\n\
         continuum_frames_encoded {enc}\n\
         # HELP continuum_frames_decoded Total frames decoded\n\
         # TYPE continuum_frames_decoded counter\n\
         continuum_frames_decoded {dec}\n",
        uptime = uptime,
        conn = state.connections.load(std::sync::atomic::Ordering::Relaxed),
        enc = state
            .frames_encoded
            .load(std::sync::atomic::Ordering::Relaxed),
        dec = state
            .frames_decoded
            .load(std::sync::atomic::Ordering::Relaxed),
    )
}
