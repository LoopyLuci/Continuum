//! A local WebSocket sidecar that streams QEMU VM frames and accepts input.
//!
//! This is the bridge between Continuum's Rust core and the PyQt5 GUI. The GUI
//! runs in the same session as the server and would otherwise have to reach
//! into Rust; instead it opens one WebSocket to `ws://127.0.0.1:8446/ws` and
//! speaks the small JSON protocol below.
//!
//! It reuses the axum WebSocket pattern already established in
//! `crate::debug_tunnel` and in the app's `cdp` module — same `WebSocketUpgrade`
//! / `on_upgrade` shape, same `axum::serve`, same error style — rather than
//! introducing a second HTTP stack.
//!
//! ## Wire protocol
//!
//! Client to server, JSON text:
//!
//! ```json
//! {"type":"config","vm":"win11","quality":80,"fps":30,"width":1280,"height":800,"input_enabled":true}
//! {"type":"subscribe","vm":"win11"}
//! {"type":"input","input_type":"key","key":"a","pressed":true}
//! {"type":"input","input_type":"mouse_move","x":640,"y":400}
//! {"type":"input","input_type":"mouse_click","button":"left","pressed":true}
//! {"type":"input","input_type":"scroll","dx":0,"dy":-1}
//! {"type":"ping","time":1712345678901}
//! {"type":"stats_request"}
//! ```
//!
//! Server to client:
//!
//! * **binary** — raw JPEG bytes, no header, no length prefix, no metadata.
//!   A GUI can hand the buffer straight to `QPixmap.loadFromData`.
//! * **text** —
//!   `{"type":"config_ack","quality":80,"fps":30,"width":1280,"height":800}`
//!   `{"type":"pong","time":1712345678901}`
//!   `{"type":"stats","frames_sent":120,"bytes_sent":18347264,"fps":29.8}`
//!   `{"type":"vm_list","vms":["win11","ubuntu"]}`
//!   `{"type":"error","message":"..."}`
//!
//! ## Robustness rules the GUI depends on
//!
//! * An unknown `type` is ignored. A GUI that is a version ahead of the server
//!   must not lose its connection over a feature it does not recognise.
//! * Malformed JSON produces `{"type":"error"}` and the connection stays open.
//!   Closing the socket on a bad frame turns a typo into a reconnect loop.
//! * A capture or input failure produces `{"type":"error"}` too. The frame loop
//!   keeps running, so a VM that is momentarily paused degrades to a frozen
//!   picture rather than a dead socket.
//! * The listener binds `127.0.0.1` only. This is an unauthenticated control
//!   channel into every VM on the host; it must not be reachable from the
//!   network, so the host half is not configurable and only the port is.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ws::WebSocket, State, WebSocketUpgrade};
use axum::routing::get;
use axum::Router;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use continuum_core::input::RemoteInputEvent;

use crate::capture_qmp::QmpCaptureBackend;
use crate::input_qmp::QmpInputInjector;

/// Default listen port. Overridable with `CONTINUUM_SIDECAR_PORT`.
pub const DEFAULT_SIDECAR_PORT: u16 = 8446;

/// Default JPEG quality when the client does not send a `config`.
pub const DEFAULT_QUALITY: u8 = 80;

/// Default capture rate when the client does not send a `config`.
pub const DEFAULT_FPS: u32 = 30;

/// Default stream size when the client does not send a `config`.
pub const DEFAULT_WIDTH: u32 = 1280;
pub const DEFAULT_HEIGHT: u32 = 800;

/// Upper bound on the requested frame rate.
///
/// An `fps` of 10000 is a bug in the GUI, and honouring it would put a
/// screendump on the wire per millisecond. Clamping turns that into a fast
/// stream instead of a saturated one.
pub const MAX_FPS: u32 = 60;

/// How long a client has to present its auth message before being dropped.
///
/// Bounded so an unauthenticated socket cannot be parked open indefinitely,
/// holding the `vm_list` catalogue hostage before it has proved anything.
pub const AUTH_TIMEOUT_SEC: u64 = 5;

/// Environment variable overriding [`DEFAULT_SIDECAR_PORT`].
pub const SIDECAR_PORT_ENV: &str = "CONTINUUM_SIDECAR_PORT";

/// Environment variable naming the VMs the sidecar may stream, as
/// `name=host:port[,name=host:port]`.
///
/// Deliberately explicit. Scanning for QEMU sockets would attach the sidecar to
/// whatever happened to be listening on 4444, including a VM that belongs to an
/// unrelated tool.
pub const SIDECAR_VMS_ENV: &str = "CONTINUUM_SIDECAR_VMS";

/// Environment variable holding the shared secret clients must present.
pub const SIDECAR_TOKEN_ENV: &str = "CONTINUUM_SIDECAR_TOKEN";

/// Mint a random token for a sidecar that was not given one.
///
/// 32 hex characters of OS randomness. Not a password: it exists so that an
/// unauthenticated local process cannot drive the guest, and so the value can
/// be logged once at startup instead of being prompted for.
pub fn generate_token() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time equality, so a client cannot discover the token by timing
/// successive guesses.
pub fn token_matches(expected: &str, supplied: &str) -> bool {
    if expected.len() != supplied.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(supplied.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn default_quality() -> u8 {
    DEFAULT_QUALITY
}
fn default_fps() -> u32 {
    DEFAULT_FPS
}
fn default_width() -> u32 {
    DEFAULT_WIDTH
}
fn default_height() -> u32 {
    DEFAULT_HEIGHT
}

/// A message from the GUI to the sidecar.
///
/// Internally tagged on `type`. The five `input_type` variants share one
/// `type` and each carries different fields, so the payload fields are all
/// optional and validated in [`translate_input`] rather than by serde. Unknown
/// types land in [`ClientMessage::Unknown`] rather than failing the whole
/// deserialisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Must be the first message on a connection. See the module docs: this
    /// sidecar injects keystrokes and pointer events into real guests, so an
    /// unauthenticated socket is a remote-control channel with no owner.
    #[serde(rename = "auth")]
    Auth { key: String },
    #[serde(rename = "config")]
    Config {
        vm: String,
        #[serde(default = "default_quality")]
        quality: u8,
        #[serde(default = "default_fps")]
        fps: u32,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
        #[serde(default)]
        input_enabled: bool,
    },
    #[serde(rename = "subscribe")]
    Subscribe { vm: String },
    #[serde(rename = "input")]
    Input {
        input_type: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        pressed: Option<bool>,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        dx: Option<i32>,
        #[serde(default)]
        dy: Option<i32>,
    },
    #[serde(rename = "ping")]
    Ping { time: u64 },
    #[serde(rename = "stats_request")]
    StatsRequest,
    /// Anything the sidecar does not implement. Ignored, connection kept.
    #[serde(other)]
    Unknown,
}

impl ClientMessage {
    /// Parse a text frame.
    ///
    /// A syntactically invalid frame is an `Err`, and the caller answers with
    /// `{"type":"error"}` rather than closing.
    pub fn parse(text: &str) -> Result<Self, String> {
        // Two failure modes, two messages: "invalid JSON" and "unrecognised
        // message" send the reader to different places.
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        serde_json::from_value(value).map_err(|e| format!("unrecognised message: {e}"))
    }

    /// The `input_type` of an input message, lowercased.
    pub fn input_kind(&self) -> Option<String> {
        match self {
            ClientMessage::Input { input_type, .. } => Some(input_type.to_ascii_lowercase()),
            _ => None,
        }
    }

    /// The `type` tag, for logging.
    pub fn kind(&self) -> &'static str {
        match self {
            ClientMessage::Auth { .. } => "auth",
            ClientMessage::Config { .. } => "config",
            ClientMessage::Subscribe { .. } => "subscribe",
            ClientMessage::Input { .. } => "input",
            ClientMessage::Ping { .. } => "ping",
            ClientMessage::StatsRequest => "stats_request",
            ClientMessage::Unknown => "unknown",
        }
    }
}

/// A message from the sidecar to the GUI.
///
/// Frames are *not* represented here: they go out as binary WebSocket messages.
/// Wrapping them in this enum would mean base64 and a 33% bandwidth penalty for
/// no benefit, and the spec is explicit that a frame is raw JPEG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "config_ack")]
    ConfigAck {
        quality: u8,
        fps: u32,
        width: u32,
        height: u32,
    },
    #[serde(rename = "pong")]
    Pong { time: u64 },
    #[serde(rename = "stats")]
    Stats {
        frames_sent: u64,
        bytes_sent: u64,
        fps: f32,
    },
    #[serde(rename = "vm_list")]
    VmList { vms: Vec<String> },
    #[serde(rename = "error")]
    Error { message: String },
}

impl ServerMessage {
    pub fn error(message: impl Into<String>) -> Self {
        ServerMessage::Error {
            message: message.into(),
        }
    }

    pub fn encode(&self) -> String {
        // No variant can fail to serialise, but panicking inside the frame
        // loop would take down a live stream over a diagnostic message.
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(
                "{{\"type\":\"error\",\"message\":\"failed to serialise response: {}\"}}",
                e
            )
        })
    }
}

/// A VM the sidecar can stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmTarget {
    pub name: String,
    pub qmp_addr: SocketAddr,
    /// QEMU `id=` of the guest's `usb-tablet`. Pointer input needs that device
    /// to exist; see `crate::input_qmp`.
    pub tablet_device: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl VmTarget {
    pub fn new(name: &str, qmp_addr: SocketAddr) -> Self {
        Self {
            name: name.to_string(),
            qmp_addr,
            tablet_device: None,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }

    pub fn with_tablet_device(mut self, device: &str) -> Self {
        self.tablet_device = Some(device.to_string());
        self
    }
}

/// Parse the `SIDECAR_VMS_ENV` value.
///
/// Accepts `name=host:port` separated by commas or semicolons, with
/// whitespace around the separators. A malformed entry is an error rather than
/// a skip: silently dropping half the VM list leaves a GUI that shows two VMs
/// and cannot explain why the third is missing.
pub fn parse_vm_list(spec: &str) -> Result<Vec<VmTarget>, String> {
    let mut targets = Vec::new();
    for entry in spec.split([',', ';']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, addr) = entry
            .split_once('=')
            .ok_or_else(|| format!("VM entry {entry:?} is not name=host:port"))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("VM entry {entry:?} has an empty name"));
        }
        let addr: SocketAddr = addr
            .trim()
            .parse()
            .map_err(|e| format!("VM {name:?} has a bad address {:?}: {e}", addr.trim()))?;
        targets.push(VmTarget::new(name, addr));
    }
    Ok(targets)
}

/// The set of VMs a sidecar may stream.
#[derive(Debug, Clone, Default)]
pub struct VmRegistry {
    targets: BTreeMap<String, VmTarget>,
}

impl VmRegistry {
    pub fn new(targets: Vec<VmTarget>) -> Self {
        let mut map = BTreeMap::new();
        for target in targets {
            map.insert(target.name.clone(), target);
        }
        Self { targets: map }
    }

    /// Build the registry from `SIDECAR_VMS_ENV`.
    ///
    /// An unset variable yields an empty registry, not a default VM: nothing
    /// says a QEMU is running, and guessing at port 4444 would attach the
    /// sidecar to something unrelated.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(SIDECAR_VMS_ENV) {
            Ok(spec) if !spec.trim().is_empty() => Ok(Self::new(parse_vm_list(&spec)?)),
            _ => Ok(Self::default()),
        }
    }

    pub fn get(&self, name: &str) -> Option<&VmTarget> {
        self.targets.get(name)
    }

    /// Registered names in sorted order, so `vm_list` is stable across calls
    /// and a GUI's picker does not reshuffle between reconnects.
    pub fn names(&self) -> Vec<String> {
        self.targets.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn insert(&mut self, target: VmTarget) {
        self.targets.insert(target.name.clone(), target);
    }
}

/// Rolling frame counters reported by `stats_request`.
#[derive(Debug, Default)]
struct StreamStats {
    frames_sent: u64,
    bytes_sent: u64,
    /// Timestamps of recent frames. An average since connect reads 30 fps
    /// while a stream that stalled ten minutes ago is still claiming it; a
    /// short window shows what is happening now.
    recent: Vec<Instant>,
}

const FPS_WINDOW: Duration = Duration::from_secs(2);

impl StreamStats {
    fn record(&mut self, bytes: usize, now: Instant) {
        self.frames_sent += 1;
        self.bytes_sent += bytes as u64;
        self.recent.push(now);
        while let Some(oldest) = self.recent.first() {
            if now.duration_since(*oldest) > FPS_WINDOW {
                self.recent.remove(0);
            } else {
                break;
            }
        }
    }

    fn fps(&self) -> f32 {
        if self.recent.len() < 2 {
            return 0.0;
        }
        let first = self.recent[0];
        let span = self.recent[self.recent.len() - 1].duration_since(first);
        if span.is_zero() {
            return 0.0;
        }
        (self.recent.len() as f32 - 1.0) / span.as_secs_f32()
    }
}

/// Per-connection mutable state.
struct Session {
    vm: Option<String>,
    backend: Option<Arc<QmpCaptureBackend>>,
    injector: Option<Arc<QmpInputInjector>>,
    quality: u8,
    fps: u32,
    width: u32,
    height: u32,
    input_enabled: bool,
    stats: StreamStats,
}

impl Session {
    fn new() -> Self {
        Self {
            vm: None,
            backend: None,
            injector: None,
            quality: DEFAULT_QUALITY,
            fps: DEFAULT_FPS,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            input_enabled: false,
            stats: StreamStats::default(),
        }
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps.clamp(1, MAX_FPS) as f64)
    }

    /// Drop the current subscription. A new `config` or `subscribe` rebuilds
    /// the backend, because the capture backend bakes the output geometry and
    /// JPEG quality in at construction.
    fn reset_backend(&mut self) {
        self.backend = None;
        self.injector = None;
        self.vm = None;
    }
}

/// The sidecar server.
#[derive(Clone)]
pub struct WsSidecar {
    registry: Arc<VmRegistry>,
    addr: SocketAddr,
    /// Shared secret every client must present before it receives frames or can
    /// inject input.
    ///
    /// This sidecar is a keyboard and mouse into every registered VM. Without a
    /// token, anything that can reach the port -- including a browser page on
    /// the host, via a WebSocket cross-origin request -- can type into and
    /// click through a guest. Loopback binding is not a defence: it stops remote
    /// hosts, not local ones.
    token: Arc<String>,
}

impl WsSidecar {
    pub fn new(registry: VmRegistry, addr: SocketAddr) -> Self {
        Self::with_token(registry, addr, &generate_token())
    }

    /// Build with a caller-supplied token.
    pub fn with_token(registry: VmRegistry, addr: SocketAddr, token: &str) -> Self {
        Self {
            registry: Arc::new(registry),
            addr,
            token: Arc::new(token.to_string()),
        }
    }

    /// The token clients must send. Logged at startup by the server so an
    /// operator can actually use the thing.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Build from the environment: port from `CONTINUUM_SIDECAR_PORT`, VMs from
    /// `CONTINUUM_SIDECAR_VMS`, token from `CONTINUUM_SIDECAR_TOKEN`.
    ///
    /// If no token is configured one is generated and returned in the error-free
    /// path; `token()` exposes it for logging. There is deliberately no way to
    /// run without a token.
    ///
    /// The host half of the address is always loopback and is not configurable
    /// — see the module docs.
    pub fn from_env() -> Result<Self, String> {
        let port = std::env::var(SIDECAR_PORT_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_SIDECAR_PORT);
        if port == 0 {
            return Err(format!("{SIDECAR_PORT_ENV} must not be 0"));
        }
        let token = std::env::var(SIDECAR_TOKEN_ENV)
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(generate_token);
        Ok(Self::with_token(
            VmRegistry::from_env()?,
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            &token,
        ))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn registry(&self) -> &VmRegistry {
        &self.registry
    }

    fn router(&self) -> Router {
Router::new()
    .route("/ws", get(handle_upgrade))
    // Same handler under the path the PyQt client and the Python bridge both
    // use, so one URL works against either server.
    .route("/ws/stream", get(handle_upgrade))
    .route("/", get(handle_index))
            .with_state(self.clone())
    }

    /// Bind and serve until the process ends.
    pub async fn serve(self) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        tracing::info!(
            addr = %self.addr,
            vms = self.registry.len(),
            "QMP sidecar listening — GUI connects to ws://{}/ws",
            self.addr
        );
        axum::serve(listener, self.router()).await?;
        Ok(())
    }
}

async fn handle_index(State(state): State<WsSidecar>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "service": "continuum-qmp-sidecar",
        "websocket": "/ws",
        "vms": state.registry.names(),
    }))
}

async fn handle_upgrade(
    State(state): State<WsSidecar>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| {
        handle_connection(socket, state.registry.clone(), state.token.clone())
    })
}

/// What the connection loop pushes to the socket.
///
/// `Outgoing` exists so the socket has exactly one writer. Input is applied on
/// a detached task, and routing its results through the same queue is what
/// keeps a 50 ms keystroke pacing delay from stalling the frame loop.
enum Outgoing {
    Text(String),
    Binary(Vec<u8>),
}

async fn handle_connection(
    mut socket: WebSocket,
    registry: Arc<VmRegistry>,
    token: Arc<String>,
) {
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();

    let mut session = Session::new();

    // ── Authenticate before anything else ──────────────────────────────────
    //
    // Nothing is sent to this client until it proves it holds the token: not
    // the VM catalogue, not a frame, and no input is applied on its behalf.
    // A wrong or missing token gets one explanatory message and a close.
    let supplied: Option<String> = {
        let mut key: Option<String> = None;
        if let Ok(Some(Ok(axum::extract::ws::Message::Text(text)))) = tokio::time::timeout(
            std::time::Duration::from_secs(AUTH_TIMEOUT_SEC),
            socket.recv(),
        )
        .await
        {
            if let Ok(ClientMessage::Auth { key: k }) = ClientMessage::parse(&text) {
                key = Some(k);
            }
        }
        key
    };

    match supplied {
        Some(key) if token_matches(&token, &key) => {}
        _ => {
            // Written straight to the socket. The outgoing queue has no writer
            // yet at this point -- the select! loop that forwards it starts
            // below -- so queueing the error and returning leaves the client
            // waiting for a message that is never sent.
            let notice = ServerMessage::Error {
                message: concat!(
                    "authentication required: first message must be ",
                    r#"{"type":"auth","key":"<token>"}"#,
                )
                .to_string(),
            }
            .encode();
            let _ = socket.send(axum::extract::ws::Message::Text(notice.into())).await;
            let _ = socket
                .send(axum::extract::ws::Message::Close(None))
                .await;
            return;
        }
    }

    // Announce the catalogue before anything is asked for, so a GUI can render
    // its VM picker without first having to guess a name.
    let _ = out_tx.send(Outgoing::Text(
        ServerMessage::VmList {
            vms: registry.names(),
        }
        .encode(),
    ));

    // `next_frame` drives the capture cadence. `None` means not subscribed.
    let mut next_frame: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            frame = socket.recv() => {
                match frame {
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        match ClientMessage::parse(&text) {
                            Ok(msg) => {
                                let replies = handle_client_message(
                                    &msg, &registry, &mut session, &input_tx,
                                ).await;
                                for reply in replies {
                                    let _ = out_tx.send(Outgoing::Text(reply.encode()));
                                }
                                if restarts_streaming(&msg) {
                                    next_frame = Some(tokio::time::Instant::now());
                                }
                            }
                            Err(message) => {
                                let _ = out_tx.send(Outgoing::Text(
                                    ServerMessage::error(message).encode(),
                                ));
                            }
                        }
                    }
                    // The GUI never sends frames, but a binary message is
                    // still not a reason to hang up.
                    Some(Ok(axum::extract::ws::Message::Binary(_)))
                    | Some(Ok(axum::extract::ws::Message::Ping(_)))
                    | Some(Ok(axum::extract::ws::Message::Pong(_))) => {}
                    Some(Ok(axum::extract::ws::Message::Close(_))) | None | Some(Err(_)) => break,
                }
            }

            Some(msg) = out_rx.recv() => {
                let sent = match msg {
                    Outgoing::Text(text) => {
                        socket.send(axum::extract::ws::Message::Text(text.into())).await
                    }
                    Outgoing::Binary(bytes) => {
                        socket.send(axum::extract::ws::Message::Binary(bytes.into())).await
                    }
                };
                if sent.is_err() {
                    break;
                }
            }

            _ = async {
                match next_frame {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                let interval = session.frame_interval();
                let Some(backend) = session.backend.clone() else {
                    // Not subscribed: park rather than spin at 60 Hz doing
                    // nothing, which would burn a core for no GUI.
                    next_frame = Some(tokio::time::Instant::now() + interval);
                    continue;
                };
                match backend.capture_jpeg().await {
                    Ok(frame) => {
                        session.stats.record(frame.jpeg.len(), Instant::now());
                        if out_tx.send(Outgoing::Binary(frame.jpeg)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        // Report and keep the loop alive: a paused VM should
                        // freeze the picture, not disconnect the GUI.
                        tracing::warn!(error = %e, "QMP capture failed");
                        let _ = out_tx.send(Outgoing::Text(
                            ServerMessage::error(format!("capture failed: {e}")).encode(),
                        ));
                    }
                }
                next_frame = Some(tokio::time::Instant::now() + interval);
            }
        }

        // Drain failures reported by the input task. Queued rather than awaited
        // inline so a rejected key cannot reorder the frame stream.
        while let Ok(msg) = input_rx.try_recv() {
            let _ = out_tx.send(Outgoing::Text(msg.encode()));
        }
    }
}

fn restarts_streaming(msg: &ClientMessage) -> bool {
    matches!(
        msg,
        ClientMessage::Subscribe { .. } | ClientMessage::Config { .. }
    )
}

/// Apply one client message, returning whatever should be sent back.
///
/// `input_tx` is how the detached input task reports failures; it is not
/// awaited here so that input cannot stall frames.
async fn handle_client_message(
    msg: &ClientMessage,
    registry: &VmRegistry,
    session: &mut Session,
    input_tx: &tokio::sync::mpsc::UnboundedSender<ServerMessage>,
) -> Vec<ServerMessage> {
    match msg {
        // Already consumed by handle_connection before any of this runs. Kept
        // exhaustive so a later variant cannot be added without being handled.
        ClientMessage::Auth { .. } => Vec::new(),
        ClientMessage::Config {
            vm,
            quality,
            fps,
            width,
            height,
            input_enabled,
        } => {
            // Clamp rather than reject: a GUI asking for 1000 fps wants a fast
            // stream, not an error dialog.
            session.quality = (*quality).clamp(1, 100);
            session.fps = (*fps).clamp(1, MAX_FPS);
            session.width = (*width).max(1);
            session.height = (*height).max(1);
            session.input_enabled = *input_enabled;
            session.reset_backend();

            let mut replies = vec![ServerMessage::ConfigAck {
                quality: session.quality,
                fps: session.fps,
                width: session.width,
                height: session.height,
            }];
            // Acknowledge first, then complain: the GUI has already applied the
            // settings it asked for and should know they took effect even if
            // the VM is unreachable.
            replies.extend(attach_vm(registry, session, vm).await);
            replies
        }

        ClientMessage::Subscribe { vm } => {
            session.reset_backend();
            attach_vm(registry, session, vm).await
        }

        ClientMessage::Input { .. } => {
            let Some(injector) = session.injector.clone() else {
                return vec![ServerMessage::error(
                    "input received before a subscribe: nothing to inject into",
                )];
            };
            if !session.input_enabled {
                return vec![ServerMessage::error(
                    "input is disabled for this session (config.input_enabled was false)",
                )];
            }
            let Some(event) = translate_input(msg) else {
                return vec![ServerMessage::error(format!(
                    "unsupported input message {:?}",
                    msg.input_kind().unwrap_or_default()
                ))];
            };
            let errors = input_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = injector.apply_async(&event).await {
                    let _ = errors.send(ServerMessage::error(format!("input failed: {e}")));
                }
            });
            Vec::new()
        }

        ClientMessage::Ping { time } => vec![ServerMessage::Pong { time: *time }],

        ClientMessage::StatsRequest => vec![ServerMessage::Stats {
            frames_sent: session.stats.frames_sent,
            bytes_sent: session.stats.bytes_sent,
            fps: session.stats.fps(),
        }],

        ClientMessage::Unknown => {
            tracing::debug!("sidecar ignored an unrecognised client message");
            Vec::new()
        }
    }
}

/// Build a `core::input::RemoteInputEvent` from an input message.
///
/// Returns `None` when the message carries no instruction — currently only a
/// key or click *release*, because HMP's `sendkey` presses and releases
/// atomically and treating a release as another tap would double every
/// keystroke a GUI sends as a press/release pair.
pub fn translate_input(msg: &ClientMessage) -> Option<RemoteInputEvent> {
    use continuum_core::input::{InputAction, MouseButton};

    let ClientMessage::Input {
        input_type,
        key,
        pressed,
        x,
        y,
        button,
        dx,
        dy,
    } = msg
    else {
        return None;
    };

    let action = match input_type.to_ascii_lowercase().as_str() {
        "key" | "mouse_click" if pressed == &Some(false) => return None,
        "key" => InputAction::KeyPress,
        "mouse_move" => InputAction::MouseMove,
        "mouse_click" => InputAction::MouseClick,
        "scroll" => InputAction::MouseScroll,
        _ => return None,
    };

    let button = button.as_deref().and_then(|b| match b.to_ascii_lowercase().as_str() {
        "left" => Some(MouseButton::Left),
        "right" => Some(MouseButton::Right),
        "middle" | "center" => Some(MouseButton::Middle),
        _ => None,
    });

    Some(RemoteInputEvent {
        action,
        x: *x,
        y: *y,
        button,
        key: key.clone(),
        modifiers: None,
        scroll_x: dx.map(|v| v as f32),
        scroll_y: dy.map(|v| v as f32),
        monitor_id: None,
    })
}

/// Connect the session's capture backend and injector to a VM.
///
/// Both share one QMP connection: a VM is reached with one handshake rather
/// than two, and every command from either path is serialised against the
/// other on that socket, which is exactly the invariant QMP requires.
async fn attach_vm(
    registry: &VmRegistry,
    session: &mut Session,
    vm: &str,
) -> Vec<ServerMessage> {
    let Some(target) = registry.get(vm).cloned() else {
        session.reset_backend();
        return vec![ServerMessage::error(format!(
            "unknown VM {vm:?}; known VMs are {:?}",
            registry.names()
        ))];
    };

    let backend = match QmpCaptureBackend::connect(&target.name, target.qmp_addr).await {
        Ok(backend) => Arc::new(backend.with_format(session.quality, session.width, session.height)),
        Err(e) => {
            session.reset_backend();
            return vec![ServerMessage::error(format!(
                "cannot reach VM {vm:?} at {}: {e}",
                target.qmp_addr
            ))];
        }
    };

    let injector = Arc::new(
        QmpInputInjector::new(
            &target.name,
            Arc::clone(backend.client()),
            target.tablet_device.clone(),
        )
        .with_guest_size(session.width, session.height),
    );

    session.backend = Some(backend);
    session.injector = Some(injector);
    session.vm = Some(vm.to_string());
    tracing::info!(vm = %vm, addr = %target.qmp_addr, "sidecar attached to VM");
    Vec::new()
}

/// Convenience view of the registry behind a lock, for callers that build the
/// sidecar from a longer-lived configuration object.
pub type SharedRegistry = Arc<RwLock<VmRegistry>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn no_mods() -> continuum_core::input::ModifierKeys {
        continuum_core::input::ModifierKeys::default()
    }

    #[test]
    fn test_config_message_round_trip() {
        let text = r#"{"type":"config","vm":"win11","quality":70,"fps":15,"width":800,"height":600,"input_enabled":true}"#;
        let msg = ClientMessage::parse(text).unwrap();
        let ClientMessage::Config {
            vm,
            quality,
            fps,
            width,
            height,
            input_enabled,
        } = &msg
        else {
            panic!("expected config, got {msg:?}");
        };
        assert_eq!(vm, "win11");
        assert_eq!(*quality, 70);
        assert_eq!(*fps, 15);
        assert_eq!(*width, 800);
        assert_eq!(*height, 600);
        assert!(*input_enabled);
        // Back to the identical wire form.
        assert_eq!(serde_json::to_string(&msg).unwrap(), text);
    }

    #[test]
    fn test_subscribe_message() {
        let msg = ClientMessage::parse(r#"{"type":"subscribe","vm":"ubuntu"}"#).unwrap();
        assert_eq!(
            msg,
            ClientMessage::Subscribe {
                vm: "ubuntu".to_string()
            }
        );
    }

    #[test]
    fn test_ping_message() {
        let msg = ClientMessage::parse(r#"{"type":"ping","time":1712345678901}"#).unwrap();
        assert_eq!(msg, ClientMessage::Ping { time: 1712345678901 });
    }

    #[test]
    fn test_stats_request_has_no_payload() {
        assert_eq!(
            ClientMessage::parse(r#"{"type":"stats_request"}"#).unwrap(),
            ClientMessage::StatsRequest
        );
    }

    #[test]
    fn test_input_messages() {
        let cases = [
            (r#"{"type":"input","input_type":"key","key":"a","pressed":true}"#, "key"),
            (
                r#"{"type":"input","input_type":"mouse_move","x":640,"y":400}"#,
                "mouse_move",
            ),
            (
                r#"{"type":"input","input_type":"mouse_click","button":"left","pressed":true}"#,
                "mouse_click",
            ),
            (
                r#"{"type":"input","input_type":"scroll","dx":0,"dy":-1}"#,
                "scroll",
            ),
        ];
        for (text, kind) in cases {
            let msg = ClientMessage::parse(text).unwrap();
            assert_eq!(msg.input_kind().as_deref(), Some(kind), "for {text}");
        }
    }

    #[test]
    fn test_unknown_type_is_ignored_not_rejected() {
        for text in [
            r#"{"type":"teleport","x":1}"#,
            r#"{"type":"something_new","nested":{"a":1}}"#,
        ] {
            assert_eq!(
                ClientMessage::parse(text).unwrap(),
                ClientMessage::Unknown,
                "{text} should parse as Unknown"
            );
            assert_eq!(ClientMessage::Unknown.kind(), "unknown");
        }
    }

    #[test]
    fn test_malformed_json_is_an_error_not_a_panic() {
        for text in ["", "not json", "{", "{\"type\":}", "[1,2,3]"] {
            assert!(
                ClientMessage::parse(text).is_err(),
                "{text:?} should not parse"
            );
        }
    }

    #[test]
    fn test_missing_type_field_is_an_error() {
        let err = ClientMessage::parse(r#"{"vm":"win11"}"#).unwrap_err();
        assert!(err.contains("unrecognised message"), "got: {err}");
    }

    #[test]
    fn test_config_fields_default_when_omitted() {
        let msg = ClientMessage::parse(r#"{"type":"config","vm":"win11"}"#).unwrap();
        let ClientMessage::Config {
            quality,
            fps,
            width,
            height,
            input_enabled,
            ..
        } = &msg
        else {
            panic!("expected config");
        };
        assert_eq!(*quality, DEFAULT_QUALITY);
        assert_eq!(*fps, DEFAULT_FPS);
        assert_eq!(*width, DEFAULT_WIDTH);
        assert_eq!(*height, DEFAULT_HEIGHT);
        assert!(!*input_enabled, "input must be off until asked for");
    }

    #[test]
    fn test_server_message_shapes() {
        assert_eq!(
            ServerMessage::ConfigAck {
                quality: 80,
                fps: 30,
                width: 1280,
                height: 800
            }
            .encode(),
            r#"{"type":"config_ack","quality":80,"fps":30,"width":1280,"height":800}"#
        );
        assert_eq!(
            ServerMessage::Pong { time: 42 }.encode(),
            r#"{"type":"pong","time":42}"#
        );
        assert_eq!(
            ServerMessage::Stats {
                frames_sent: 3,
                bytes_sent: 1024,
                fps: 29.5
            }
            .encode(),
            r#"{"type":"stats","frames_sent":3,"bytes_sent":1024,"fps":29.5}"#
        );
        assert_eq!(
            ServerMessage::VmList {
                vms: vec!["a".into(), "b".into()]
            }
            .encode(),
            r#"{"type":"vm_list","vms":["a","b"]}"#
        );
        assert_eq!(
            ServerMessage::error("boom").encode(),
            r#"{"type":"error","message":"boom"}"#
        );
    }

    #[test]
    fn test_server_messages_deserialise_back() {
        for msg in [
            ServerMessage::ConfigAck {
                quality: 1,
                fps: 2,
                width: 3,
                height: 4,
            },
            ServerMessage::Pong { time: 5 },
            ServerMessage::Stats {
                frames_sent: 6,
                bytes_sent: 7,
                fps: 8.0,
            },
            ServerMessage::VmList {
                vms: vec!["x".into()],
            },
            ServerMessage::error("e"),
        ] {
            let text = msg.encode();
            let back: ServerMessage = serde_json::from_str(&text).unwrap();
            assert_eq!(back, msg, "round trip failed for {text}");
        }
    }

    #[test]
    fn test_vm_list_parsing() {
        let targets = parse_vm_list("win11=127.0.0.1:4444, ubuntu = 127.0.0.1:4445").unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "win11");
        assert_eq!(targets[0].qmp_addr.to_string(), "127.0.0.1:4444");
        assert_eq!(targets[1].name, "ubuntu");
        assert_eq!(targets[1].qmp_addr.to_string(), "127.0.0.1:4445");
    }

    #[test]
    fn test_vm_list_parsing_rejects_garbage() {
        assert!(parse_vm_list("win11").is_err(), "missing =");
        assert!(parse_vm_list("=127.0.0.1:4444").is_err(), "empty name");
        assert!(parse_vm_list("win11=notanaddress").is_err(), "bad address");
    }

    #[test]
    fn test_registry_names_are_sorted_and_stable() {
        let registry = VmRegistry::new(vec![
            VmTarget::new("ubuntu", "127.0.0.1:4445".parse().unwrap()),
            VmTarget::new("win11", "127.0.0.1:4444".parse().unwrap()),
        ]);
        assert_eq!(registry.names(), vec!["ubuntu", "win11"]);
        assert_eq!(registry.names(), registry.names());
        assert_eq!(registry.len(), 2);
        assert!(registry.get("win11").is_some());
        assert!(registry.get("nope").is_none());
    }

    #[test]
    fn test_empty_registry_lists_nothing() {
        let registry = VmRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(
            ServerMessage::VmList {
                vms: registry.names()
            }
            .encode(),
            r#"{"type":"vm_list","vms":[]}"#
        );
    }

    #[test]
    fn test_sidecar_addr_is_loopback_only() {
        // Loopback is not configurable: a sidecar reachable from the LAN hands
        // remote input to every VM on the host.
        let sidecar = WsSidecar::new(VmRegistry::default(), "127.0.0.1:8446".parse().unwrap());
        assert!(sidecar.addr().ip().is_loopback());
        assert_eq!(sidecar.addr().port(), DEFAULT_SIDECAR_PORT);
    }

    #[test]
    fn test_default_port_is_8446() {
        assert_eq!(DEFAULT_SIDECAR_PORT, 8446);
    }

    #[test]
    fn test_stream_stats_counts_and_rate() {
        let mut stats = StreamStats::default();
        let now = Instant::now();
        for i in 0..10u64 {
            stats.record(1000, now + Duration::from_millis(i * 100));
        }
        assert_eq!(stats.frames_sent, 10);
        assert_eq!(stats.bytes_sent, 10_000);
        // Nine intervals over 900 ms is 10 fps.
        let fps = stats.fps();
        assert!((fps - 10.0).abs() < 0.5, "fps was {fps}");
    }

    #[test]
    fn test_stream_stats_fps_is_zero_with_one_frame() {
        let mut stats = StreamStats::default();
        stats.record(10, Instant::now());
        assert_eq!(stats.fps(), 0.0, "one frame is not a rate");
    }

    #[test]
    fn test_stream_stats_drops_frames_outside_the_window() {
        let mut stats = StreamStats::default();
        let now = Instant::now();
        stats.record(1, now);
        stats.record(1, now + Duration::from_secs(30));
        stats.record(1, now + Duration::from_secs(31));
        assert_eq!(stats.frames_sent, 3);
        assert_eq!(stats.recent.len(), 2, "the stale frame should be gone");
    }

    #[test]
    fn test_frame_interval_is_clamped_to_max_fps() {
        let mut session = Session::new();
        assert_eq!(
            session.frame_interval(),
            Duration::from_secs_f64(1.0 / DEFAULT_FPS as f64)
        );
        session.fps = 10_000;
        let interval = session.frame_interval().as_secs_f64();
        assert!(
            (interval - 1.0 / MAX_FPS as f64).abs() < 1e-6,
            "interval was {interval}"
        );
        session.fps = 0;
        assert!(session.frame_interval().as_secs_f64() <= 1.0);
    }

    #[test]
    fn test_new_session_does_not_capture_or_accept_input() {
        let session = Session::new();
        assert!(session.backend.is_none());
        assert!(session.injector.is_none());
        assert!(session.vm.is_none());
        assert!(!session.input_enabled);
    }

    #[test]
    fn test_reset_backend_clears_everything() {
        let mut session = Session::new();
        session.vm = Some("win11".into());
        session.input_enabled = true;
        session.reset_backend();
        assert!(session.backend.is_none());
        assert!(session.injector.is_none());
        assert!(session.vm.is_none());
        // Configuration survives; only the attachment is torn down.
        assert!(session.input_enabled);
    }

    #[test]
    fn test_translate_key_event() {
        let msg =
            ClientMessage::parse(r#"{"type":"input","input_type":"key","key":"a","pressed":true}"#)
                .unwrap();
        let event = translate_input(&msg).expect("should translate");
        assert_eq!(event.action, continuum_core::input::InputAction::KeyPress);
        assert_eq!(event.key.as_deref(), Some("a"));
    }

    #[test]
    fn test_key_release_carries_no_instruction() {
        // sendkey is atomic; a release must not become a second tap.
        let msg =
            ClientMessage::parse(r#"{"type":"input","input_type":"key","key":"a","pressed":false}"#)
                .unwrap();
        assert!(translate_input(&msg).is_none());
    }

    #[test]
    fn test_translate_mouse_move() {
        let msg =
            ClientMessage::parse(r#"{"type":"input","input_type":"mouse_move","x":100,"y":-5}"#)
                .unwrap();
        let event = translate_input(&msg).unwrap();
        assert_eq!(event.x, Some(100));
        assert_eq!(event.y, Some(-5));
        assert_eq!(event.action, continuum_core::input::InputAction::MouseMove);
    }

    #[test]
    fn test_translate_scroll_carries_deltas() {
        let msg = ClientMessage::parse(r#"{"type":"input","input_type":"scroll","dx":1,"dy":-3}"#)
            .unwrap();
        let event = translate_input(&msg).unwrap();
        assert_eq!(event.scroll_x, Some(1.0));
        assert_eq!(event.scroll_y, Some(-3.0));
        assert_eq!(
            event.action,
            continuum_core::input::InputAction::MouseScroll
        );
    }

    #[test]
    fn test_translate_rejects_unknown_input_type() {
        let msg = ClientMessage::parse(r#"{"type":"input","input_type":"telekinesis"}"#).unwrap();
        assert!(translate_input(&msg).is_none());
    }

    #[test]
    fn test_translate_rejects_non_input_message() {
        assert!(translate_input(&ClientMessage::Ping { time: 1 }).is_none());
        assert!(translate_input(&ClientMessage::StatsRequest).is_none());
        assert!(translate_input(&ClientMessage::Unknown).is_none());
    }

    #[test]
    fn test_button_names_parse_and_bad_ones_are_dropped() {
        for (wire, expected) in [
            ("left", Some(continuum_core::input::MouseButton::Left)),
            ("right", Some(continuum_core::input::MouseButton::Right)),
            ("middle", Some(continuum_core::input::MouseButton::Middle)),
            ("center", Some(continuum_core::input::MouseButton::Middle)),
            ("LEFT", Some(continuum_core::input::MouseButton::Left)),
            ("thumb", None),
        ] {
            let msg = ClientMessage::parse(&format!(
                r#"{{"type":"input","input_type":"mouse_click","button":"{wire}","pressed":true}}"#
            ))
            .unwrap();
            let event = translate_input(&msg).unwrap();
            assert_eq!(event.button, expected, "for button {wire}");
        }
    }

    #[test]
    fn test_only_config_and_subscribe_restart_the_stream() {
        assert!(restarts_streaming(&ClientMessage::Config {
            vm: "v".into(),
            quality: 1,
            fps: 1,
            width: 1,
            height: 1,
            input_enabled: false,
        }));
        assert!(restarts_streaming(&ClientMessage::Subscribe {
            vm: "v".into()
        }));
        assert!(!restarts_streaming(&ClientMessage::Ping { time: 0 }));
        assert!(!restarts_streaming(&ClientMessage::StatsRequest));
        assert!(!restarts_streaming(&ClientMessage::Unknown));
    }

    #[test]
    fn test_config_clamps_out_of_range_values() {
        let registry = VmRegistry::default();
        let mut session = Session::new();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let msg = ClientMessage::parse(
            r#"{"type":"config","vm":"nope","quality":0,"fps":9999,"width":0,"height":0}"#,
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let replies = runtime.block_on(handle_client_message(
            &msg, &registry, &mut session, &input_tx,
        ));
        assert_eq!(session.quality, 1, "quality clamped into 1..=100");
        assert_eq!(session.fps, MAX_FPS);
        assert_eq!(session.width, 1, "width floored at 1");
        assert_eq!(session.height, 1, "height floored at 1");
        // The ack reports the clamped values, not what was asked for.
        assert!(replies.iter().any(|r| matches!(
            r,
            ServerMessage::ConfigAck { fps, quality, width, height }
                if *fps == MAX_FPS && *quality == 1 && *width == 1 && *height == 1
        )));
        // And the unknown VM is reported separately.
        assert!(replies.iter().any(|r| matches!(
            r,
            ServerMessage::Error { message } if message.contains("unknown VM")
        )));
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn test_negative_size_is_a_protocol_error_not_a_clamp() {
        // Clamping is for values the protocol allows but the stream cannot
        // honour. A negative height is a type violation, and the spec is
        // explicit that malformed input gets an `error` rather than a guess.
        let err = ClientMessage::parse(
            r#"{"type":"config","vm":"win11","quality":80,"fps":30,"width":-1,"height":600}"#,
        )
        .unwrap_err();
        assert!(err.contains("unrecognised message"), "got: {err}");
    }

    #[test]
    fn test_input_before_subscribe_is_refused_not_applied() {
        let registry = VmRegistry::new(vec![VmTarget::new(
            "win11",
            "127.0.0.1:1".parse().unwrap(),
        )]);
        let mut session = Session::new();
        // No backend attached: the QMP handshake was never attempted.
        let (input_tx, _input_rx) = tokio::sync::mpsc::unbounded_channel();
        let msg = ClientMessage::parse(
            r#"{"type":"input","input_type":"key","key":"a","pressed":true}"#,
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let replies = runtime.block_on(handle_client_message(
            &msg, &registry, &mut session, &input_tx,
        ));
        assert!(matches!(
            replies.as_slice(),
            [ServerMessage::Error { message }] if message.contains("before a subscribe")
        ));
    }

    #[test]
    fn test_ping_and_stats_need_no_vm() {
        let registry = VmRegistry::default();
        let mut session = Session::new();
        let (input_tx, _input_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let ping = ClientMessage::parse(r#"{"type":"ping","time":99}"#).unwrap();
        assert_eq!(
            runtime.block_on(handle_client_message(&ping, &registry, &mut session, &input_tx)),
            vec![ServerMessage::Pong { time: 99 }]
        );

        let stats = ClientMessage::parse(r#"{"type":"stats_request"}"#).unwrap();
        assert_eq!(
            runtime.block_on(handle_client_message(
                &stats, &registry, &mut session, &input_tx
            )),
            vec![ServerMessage::Stats {
                frames_sent: 0,
                bytes_sent: 0,
                fps: 0.0
            }]
        );
    }

    #[test]
    fn test_unknown_message_produces_no_reply() {
        let registry = VmRegistry::default();
        let mut session = Session::new();
        let (input_tx, _input_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let msg = ClientMessage::parse(r#"{"type":"from_the_future"}"#).unwrap();
        assert!(runtime
            .block_on(handle_client_message(&msg, &registry, &mut session, &input_tx))
            .is_empty());
    }

    #[test]
    fn test_vm_target_carries_tablet_device() {
        let target = VmTarget::new("win11", "127.0.0.1:4444".parse().unwrap())
            .with_tablet_device("tablet0");
        assert_eq!(target.tablet_device.as_deref(), Some("tablet0"));
        assert_eq!(target.width, DEFAULT_WIDTH);
        assert_eq!(target.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn test_modifier_default_is_all_false() {
        // Guards the translation default: the sidecar sends no modifier field,
        // so a stray default here would shift every keystroke.
        let mods = no_mods();
        assert!(!mods.ctrl && !mods.alt && !mods.shift && !mods.super_key);
    }
}