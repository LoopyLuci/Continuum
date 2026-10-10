//! The RFB listener, the per-client state machine, the update loop and input.
//!
//! ## Task layout
//!
//! ```text
//! serve()                       one accept loop
//!   └── client task (per socket) handshake, then a select! over three sources
//!         ├── reader:  client messages -> input, encoding and request state
//!         ├── frames:  a captured frame from the shared capture loop
//!         └── writer:  a detached task holding the socket's write half
//! ```
//!
//! One task owns the read half, one owns the write half, and nothing else touches
//! the socket. That is what makes the slow-client policy possible: the writer is
//! fed by a bounded channel, so a client that stops reading fills its queue,
//! loses frames, and never applies back-pressure to the capture loop or to any
//! other client.
//!
//! ## Damage and `CopyRect` correctness
//!
//! Each client keeps its own [`Framebuffer`] — what *that* client holds — while
//! the capture loop publishes one shared `Arc<Framebuffer>` per poll: what the
//! guest looks like now. The per-client copy is what makes the invariant
//! checkable. Sending `CopyRect` asserts that the client holds the source pixels,
//! which is only true if the client's view is byte-identical to the current frame
//! outside the rectangles being sent. That is maintained deliberately:
//!
//! * a client that connects late, or that asks for a non-incremental update, has
//!   its whole view invalidated and is sent the full framebuffer;
//! * a client whose write queue overflows, or that fell behind the capture loop,
//!   missed a frame and is likewise invalidated, so the next update
//!   resynchronises instead of compounding the error;
//! * a resize invalidates everything and is announced with `DesktopSize`.
//!
//! Per-client state costs `width * height * 4` bytes, which is why
//! [`RfbServerConfig::max_clients`] and [`crate::rfb::MAX_FRAMEBUFFER_PIXELS`] are
//! both bounded rather than open-ended.
//!
//! ## What this does not do
//!
//! * No TLS, no `VeNCrypt`, no Apple Authentication. VNC Authentication only.
//! * No clipboard. `ClientCutText` is accepted and logged, never applied to a
//!   guest: piping a peer's clipboard into a real machine needs an owner for that
//!   decision and there is none here.
//! * No cursor pseudo-encoding, so the client's own cursor is drawn by the client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::time::timeout;

use continuum_core::input::{InputAction, ModifierKeys, MouseButton, RemoteInputEvent};

use crate::capture_source::CaptureSource;
use crate::input_qmp::{control_key, key_for, resolve_sendkey, QmpInputInjector};

use crate::rfb::auth::{generate_challenge, VncPassword, RESPONSE_LEN};
use crate::rfb::encoder::{
    covers_most_of, full_frame, hextile_aligned, split_width, total_area, write_copy_rect,
    ChangedRegion, Encoding, EncodingPreferences, Framebuffer, Scroll, MAX_RECTS_PER_UPDATE,
    MAX_TIGHT_RECT_WIDTH,
};
use crate::rfb::proto::{
    decode_client_message, decide_version, negotiate_security_type, parse_client_version,
    security_type_offer, write_framebuffer_update_header, write_security_result, ClientMessage,
    CopyRect, PixelFormat, SecurityType, ServerInit, VersionDecision, DEFAULT_DESKTOP_NAME,
    MAX_CUT_TEXT_BYTES, MAX_VERSION_BANNER_BYTES, SECURITY_NONE, SECURITY_VNC_AUTH,
    SERVER_VERSION_BANNER,
};
use crate::rfb::{MAX_FRAMEBUFFER_DIMENSION, MAX_FRAMEBUFFER_PIXELS};

/// Default listen port.
pub const DEFAULT_PORT: u16 = 5900;

/// How long a client gets for the whole handshake.
///
/// Bounded so an unauthenticated socket cannot be parked open indefinitely,
/// holding a client slot and a framebuffer's worth of memory before it has
/// proved anything.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Most times the banner is re-sent to a client that asked for a newer version.
pub const MAX_VERSION_RETRIES: u32 = 2;

/// Most simultaneous clients. Four is generous for one guest's console and keeps
/// the per-client framebuffer cost bounded and predictable.
pub const DEFAULT_MAX_CLIENTS: usize = 4;

/// A client that sends nothing for this long is disconnected.
///
/// A VNC client can legitimately sit idle for minutes — a user reading a document
/// without touching the mouse — so this is generous by design. It exists to
/// reclaim sockets from a client that vanished without a FIN, which on a
/// long-lived listener is the difference between bounded and unbounded.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Encoded updates a client may have queued before its frames start dropping.
///
/// Shallow on purpose. The queue exists to absorb a scheduling hiccup, not to
/// build a backlog: a backlog on a live desktop is stale by the time it is
/// written.
pub const DEFAULT_WRITE_QUEUE_DEPTH: usize = 4;

/// Queued input events per client before input starts being dropped.
///
/// Bounded on purpose: a client that floods pointer moves must not be able to
/// grow this without limit. Frames outrank input, so a full queue loses input.
pub const INPUT_QUEUE_DEPTH: usize = 256;

/// Depth of the broadcast channel carrying captured frames to clients.
const FRAME_CHANNEL_DEPTH: usize = 2;

/// Poll interval for the capture loop when the caller does not set one.
///
/// Sixty-six milliseconds, which is roughly what a `screendump` round trip costs
/// (see [`crate::capture_qmp`]). Polling faster cannot make the guest produce
/// frames any sooner; it can only queue screendumps behind each other.
pub const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(66);

/// Errors from the RFB server.
#[derive(Debug, thiserror::Error)]
pub enum RfbError {
    #[error("RFB I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),

    /// A client message this server cannot parse.
    #[error("unrecognised client message: {0}")]
    Undecodable(String),

    /// A client message type this server does not implement.
    ///
    /// Distinct from [`RfbError::Undecodable`] because it is a decoding result
    /// rather than a failure: RFC 6143 7.5 requires an unknown *type* to be
    /// ignored rather than treated as an error, so that a client one version
    /// ahead keeps its session.
    #[error("ignored client message type {0}")]
    IgnoredMessage(u8),

    #[error("authentication failed")]
    AuthFailed,

    #[error("too many clients; the limit is {0}")]
    TooManyClients(usize),

    #[error(
        "a {width}x{height} framebuffer is refused: the limits are {max_pixels} pixels \
         and {max_dimension} per side"
    )]
    FramebufferTooLarge {
        width: u32,
        height: u32,
        max_pixels: u32,
        max_dimension: u32,
    },

    #[error("invalid VNC password: {0}")]
    Password(#[from] crate::rfb::auth::PasswordError),
}

/// How the server behaves. Every bound is explicit and every default is safe.
#[derive(Clone)]
pub struct RfbServerConfig {
    /// Address to listen on. Defaults to loopback; changing the host half is a
    /// deliberate decision and this server will not make it for you.
    pub bind: SocketAddr,

    /// The VNC password. At most eight bytes — RFC 6143 truncates longer ones
    /// silently, so this is rejected rather than quietly ignored.
    pub password: String,

    /// Permit the `None` security type, i.e. no authentication at all.
    ///
    /// Off by default, and there is no good reason to turn it on for a server
    /// reachable from another host: an unauthenticated RFB connection is
    /// unrestricted keyboard and mouse control of the guest with no credential
    /// and no audit trail. It exists for a loopback-only debugging session.
    pub allow_none_auth: bool,

    /// Simultaneous clients. Each one costs a full framebuffer.
    pub max_clients: usize,

    /// Disconnect a client that has sent nothing for this long. Zero disables.
    pub idle_timeout: Duration,

    /// Queued encoded updates per client before frames start being dropped.
    pub write_queue_depth: usize,

    /// How often to ask the capture backend for a frame.
    pub frame_interval: Duration,

    /// Pixel format advertised in `ServerInit` and used until a client overrides
    /// it with `SetPixelFormat`.
    pub pixel_format: PixelFormat,

    /// Desktop name shown to clients.
    pub desktop_name: String,

    /// Accept pointer and keyboard input at all.
    pub allow_input: bool,
}

impl std::fmt::Debug for RfbServerConfig {
    /// Hand-written so the password can never reach a log line through `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RfbServerConfig")
            .field("bind", &self.bind)
            .field("password", &"<redacted>")
            .field("allow_none_auth", &self.allow_none_auth)
            .field("max_clients", &self.max_clients)
            .field("idle_timeout", &self.idle_timeout)
            .field("write_queue_depth", &self.write_queue_depth)
            .field("frame_interval", &self.frame_interval)
            .field("pixel_format", &self.pixel_format)
            .field("desktop_name", &self.desktop_name)
            .field("allow_input", &self.allow_input)
            .finish()
    }
}

impl RfbServerConfig {
    /// Loopback, authenticated, bounded defaults.
    pub fn secure(bind: SocketAddr, password: &str) -> Self {
        Self {
            bind,
            password: password.to_string(),
            allow_none_auth: false,
            max_clients: DEFAULT_MAX_CLIENTS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            write_queue_depth: DEFAULT_WRITE_QUEUE_DEPTH,
            frame_interval: DEFAULT_FRAME_INTERVAL,
            pixel_format: PixelFormat::BGRX32,
            desktop_name: DEFAULT_DESKTOP_NAME.to_string(),
            allow_input: true,
        }
    }

    /// Validate the configuration, rejecting combinations that cannot work.
    ///
    /// Called before the socket is bound, so a misconfiguration is a startup
    /// failure with an explanation rather than a client-facing protocol error.
    pub fn validate(&self) -> Result<(), RfbError> {
        // Surfaces an empty or over-long password as a startup error rather than
        // as a server that quietly authenticates nobody.
        VncPassword::new(&self.password)?;
        self.pixel_format.validate()?;
        if self.max_clients == 0 {
            return Err(RfbError::Protocol("max_clients must be at least 1".into()));
        }
        if self.write_queue_depth == 0 {
            return Err(RfbError::Protocol(
                "write_queue_depth must be at least 1".into(),
            ));
        }
        Ok(())
    }
}

/// A bounded count of connected clients.
///
/// Its own type rather than a bare `AtomicUsize` so that the limit travels with
/// the count, and so the admission rule can be unit tested without a socket, a
/// capture backend or a runtime.
struct AdmissionSlots {
    limit: usize,
    count: AtomicUsize,
}

impl AdmissionSlots {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicUsize::new(0),
        }
    }

    /// Claim a slot, or return `false` when the limit is already reached.
    ///
    /// A compare-exchange loop rather than a `fetch_add` followed by a check:
    /// the latter would let N concurrent clients all observe "one over the
    /// limit" and none of them back out, which is exactly the case a
    /// flood of simultaneous connections produces.
    fn claim(&self) -> bool {
        let mut current = self.count.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return false;
            }
            match self.count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    fn release(&self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }

    fn in_use(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

/// State shared by every client of one guest.
struct Shared {
    config: RfbServerConfig,
    capture: Arc<CaptureSource>,
    injector: Arc<QmpInputInjector>,
    frames: broadcast::Sender<Arc<Framebuffer>>,
    slots: AdmissionSlots,
    captures: AtomicU64,
}

/// Decrements the connected-client count when dropped, however the task ends.
struct AdmissionGuard(Arc<Shared>);

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.0.slots.release();
    }
}

/// An RFB server for one guest's framebuffer.
#[derive(Clone)]
pub struct RfbServer {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for RfbServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RfbServer")
            .field("config", &self.shared.config)
            .field("clients", &self.shared.slots.in_use())
            .finish_non_exhaustive()
    }
}

impl RfbServer {
    /// Build a server around an already-connected capture backend and injector.
    pub fn new(
        config: RfbServerConfig,
        capture: Arc<CaptureSource>,
        injector: Arc<QmpInputInjector>,
    ) -> Self {
        let (frames, _) = broadcast::channel(FRAME_CHANNEL_DEPTH);
        let slots = AdmissionSlots::new(config.max_clients);
        Self {
            shared: Arc::new(Shared {
                config,
                capture,
                injector,
                frames,
                slots,
                captures: AtomicU64::new(0),
            }),
        }
    }

    pub fn config(&self) -> &RfbServerConfig {
        &self.shared.config
    }

    pub fn addr(&self) -> SocketAddr {
        self.shared.config.bind
    }

    /// How many frames the capture loop has published. A cheap liveness signal
    /// for a caller that wants one.
    pub fn captures(&self) -> u64 {
        self.shared.captures.load(Ordering::Relaxed)
    }

    /// Bind the listener and return it.
    ///
    /// Separate from [`RfbServer::serve`] so a caller — or a test — can learn the
    /// port that was actually bound before anything connects to it.
    pub async fn bind(&self) -> Result<TcpListener, RfbError> {
        self.shared.config.validate()?;
        let listener = TcpListener::bind(self.shared.config.bind).await?;
        tracing::info!(
            addr = %self.shared.config.bind,
            auth = if self.shared.config.allow_none_auth { "none" } else { "vnc-auth" },
            clients = self.shared.config.max_clients,
            "RFB server listening"
        );
        Ok(listener)
    }

    /// Serve until the process ends.
    pub async fn serve(&self) -> Result<(), RfbError> {
        let listener = self.bind().await?;
        self.serve_with(listener).await
    }

    /// Serve on an already-bound listener.
    pub async fn serve_with(&self, listener: TcpListener) -> Result<(), RfbError> {
        let _capture_task = tokio::spawn(capture_loop(Arc::clone(&self.shared)));

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let shared = Arc::clone(&self.shared);
                    tokio::spawn(async move {
                        let admitted = AdmissionGuard::acquire(&shared);
                        let Some(_guard) = admitted else {
                            tracing::warn!(
                                %peer,
                                limit = shared.config.max_clients,
                                "refused an RFB connection: client limit reached"
                            );
                            return;
                        };
                        if let Err(e) = handle_client(stream, peer, &shared).await {
                            tracing::debug!(%peer, error = %e, "RFB client disconnected");
                        }
                    });
                }
                // One failed accept must not take the listener down: a transient
                // error from a socket being torn down is routine.
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

impl AdmissionGuard {
    /// Admit a client if there is room, returning the guard that releases the
    /// slot. `None` means the limit was reached.
    fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        shared.slots.claim().then(|| Self(Arc::clone(shared)))
    }
}

/// Poll the capture backend and publish each frame to the clients.
///
/// One loop for all clients, so N viewers cost one `screendump` per tick rather
/// than N. `broadcast` is what makes the slow-client policy free: a receiver that
/// falls behind is told how many frames it missed instead of being made to wait
/// for them, and the capture loop never blocks on a socket.
async fn capture_loop(shared: Arc<Shared>) {
    let interval = shared.config.frame_interval;
    loop {
        match shared.capture.capture_jpeg().await {
            Ok(frame) => match to_bgra32(&frame.jpeg, frame.width, frame.height) {
                Ok(pixels) => {
                    let width = clamp_dimension(frame.width);
                    let height = clamp_dimension(frame.height);
                    match Framebuffer::from_bgra32(width, height, pixels) {
                        Ok(frame) => {
                            // A send error only means nobody is listening.
                            let _ = shared.frames.send(Arc::new(frame));
                            shared.captures.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => tracing::warn!(error = %e, "captured frame did not match its size"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not decode a captured frame"),
            },
            Err(e) => {
                // A paused or wedged VM freezes the picture rather than dropping
                // the client, which is what a viewer expects.
                tracing::debug!(error = %e, "capture failed");
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Clamp a guest-reported dimension into the range RFB can describe.
fn clamp_dimension(value: u32) -> u16 {
    value.clamp(1, MAX_FRAMEBUFFER_DIMENSION.min(u32::from(u16::MAX))) as u16
}

/// Decode a captured JPEG into BGRX32.
///
/// `image` is already a direct dependency with the `jpeg` feature, so this adds
/// nothing. The JPEG is produced by the existing capture backend and is
/// discarded immediately: RFB never ships a whole frame, only deltas, which is
/// the entire reason this module exists.
fn to_bgra32(jpeg: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG decode failed: {e}"))?;
    let rgba = decoded.to_rgba8();
    if rgba.width() != width || rgba.height() != height {
        return Err(format!(
            "the decoded frame is {}x{} but the capture reported {width}x{height}",
            rgba.width(),
            rgba.height()
        ));
    }
    let mut bgra = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Ok(bgra)
}

/// Everything one connected client knows about itself.
#[derive(Clone)]
struct ClientState {
    /// What this client holds. The basis of every `CopyRect` correctness claim.
    view: Framebuffer,
    format: PixelFormat,
    preferences: EncodingPreferences,
    /// Set when the client's view can no longer be trusted to match the frame it
    /// was last sent, so the next update must resynchronise the whole frame.
    needs_full_refresh: Arc<AtomicBool>,
    /// An outstanding request. RFB updates are demand-driven, so nothing is sent
    /// until the client asks.
    pending: Option<PendingRequest>,
}

/// A `FramebufferUpdateRequest`, clipped to the framebuffer before use.
#[derive(Debug, Clone, Copy)]
struct PendingRequest {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl PendingRequest {
    /// The requested rectangle, clipped to the framebuffer.
    ///
    /// A request that reaches past the framebuffer edge is treated as "the whole
    /// frame". That is what keeps a client which has not processed a
    /// `DesktopSize` in sync, rather than leaving it stale forever.
    fn clip(&self, frame_width: u16, frame_height: u16) -> Option<ChangedRegion> {
        if self.width == 0 || self.height == 0 {
            // A zero-area request asks for nothing. Answering with the whole
            // frame would let a client bypass the incremental protocol with a
            // one-byte trick.
            return None;
        }
        let (frame_right, frame_bottom) = (u32::from(frame_width), u32::from(frame_height));
        let (left, top) = (u32::from(self.x), u32::from(self.y));
        if left >= frame_right || top >= frame_bottom {
            // Entirely off-screen. Nothing to do.
            return None;
        }
        let right = left + u32::from(self.width);
        let bottom = top + u32::from(self.height);
        let clipped_right = right.min(frame_right);
        let clipped_bottom = bottom.min(frame_bottom);
        let region = ChangedRegion {
            x: left as u16,
            y: top as u16,
            width: (clipped_right - left) as u16,
            height: (clipped_bottom - top) as u16,
        };
        (region.width > 0 && region.height > 0).then_some(region)
    }
}

/// Serve one client from handshake to disconnect.
async fn handle_client(
    stream: TcpStream,
    peer: SocketAddr,
    shared: &Arc<Shared>,
) -> Result<(), RfbError> {
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();

    let (width, height) = match timeout(HANDSHAKE_TIMEOUT, handshake(&mut reader, &mut writer, shared))
        .await
    {
        Ok(result) => {
            result?;
            // The capture backend bakes its geometry in at construction, so the
            // advertised size is the configured one rather than whatever a
            // resize since construction produced.
            framebuffer_geometry(&shared.capture)?
        }
        Err(_) => return Err(RfbError::Protocol("handshake timed out".into())),
    };

    let init = ServerInit {
        width,
        height,
        pixel_format: shared.config.pixel_format,
        name: shared.config.desktop_name.clone(),
    };
    let mut writer = writer;
    writer.write_all(&init.encode()).await?;
    writer.flush().await?;

    let mut state = ClientState {
        view: Framebuffer::new(width, height),
        format: shared.config.pixel_format,
        preferences: EncodingPreferences::new(&[]),
        // Nothing has been sent yet, so the client holds nothing.
        needs_full_refresh: Arc::new(AtomicBool::new(true)),
        pending: None,
    };

    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(shared.config.write_queue_depth);
    let writer_task = tokio::spawn(write_loop(writer, out_rx));

    // The read half gets its own task. This is not a stylistic choice: a read
    // future that is dropped part-way through a message has already consumed
    // those bytes from the socket, so anything that can win a `select!` race —
    // and a captured frame arriving every 30 ms always can — would silently eat
    // client messages split across TCP segments. A dedicated task owns the
    // partial-message buffer, so a message survives any number of intervening
    // frames.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(64);
    let reader_task = tokio::spawn(reader_task(reader, event_tx));

    // Input gets a third task. Every QMP command is a socket round trip behind a
    // mutex that the capture loop also holds, and `sendkey` additionally waits
    // out a 50 ms pacing delay; applying input inline would stall this client's
    // framebuffer updates for the duration of a keystroke.
    let (input_tx, input_rx) = tokio::sync::mpsc::channel::<InputCommand>(INPUT_QUEUE_DEPTH);
    let input_task = tokio::spawn(input_task(
        Arc::clone(&shared.injector),
        input_rx,
        peer,
    ));

    let result = session_loop(
        &mut state,
        &out_tx,
        event_rx,
        &input_tx,
        shared.frames.subscribe(),
        shared,
        peer,
    )
    .await;

    drop(out_tx);
    drop(input_tx);
    let _ = writer_task.await;
    let _ = input_task.await;
    reader_task.abort();
    result
}

/// The write half's only task, so nothing else can interleave with the socket.
async fn write_loop(mut writer: OwnedWriteHalf, mut out_rx: tokio::sync::mpsc::Receiver<Vec<u8>>) {
    while let Some(message) = out_rx.recv().await {
        if writer.write_all(&message).await.is_err() {
            break;
        }
    }
    let _ = writer.flush().await;
}

/// Something the read half produced.
enum ClientEvent {
    Message(ClientMessage),
    /// The client closed its side cleanly.
    Closed,
    /// A framing or protocol failure; the session ends.
    Failed(String),
}

/// The read half's only task.
///
/// Holds the partial-message buffer across reads, so a message split across TCP
/// segments is assembled rather than dropped.
async fn reader_task(mut reader: OwnedReadHalf, tx: tokio::sync::mpsc::Sender<ClientEvent>) {
    let mut buffer: Vec<u8> = Vec::with_capacity(64);
    loop {
        let total = match framed_length(&buffer) {
            Ok(Some(total)) => total,
            Ok(None) => {
                let mut chunk = [0u8; 4096];
                match reader.read(&mut chunk).await {
                    Ok(0) => {
                        let _ = tx.send(ClientEvent::Closed).await;
                        return;
                    }
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    Err(e) => {
                        let _ = tx.send(ClientEvent::Failed(e.to_string())).await;
                        return;
                    }
                }
                continue;
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::Failed(e.to_string())).await;
                return;
            }
        };

        match decode_client_message(&buffer[..total]) {
            Ok((message, consumed)) => {
                debug_assert_eq!(consumed, total, "framing and decoding must agree");
                buffer.drain(..total);
                if tx.send(ClientEvent::Message(message)).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::Failed(e.to_string())).await;
                return;
            }
        }
    }
}

/// A unit of input work, queued rather than applied inline.
enum InputCommand {
    Key { keysym: u32, down: bool },
    Pointer { x: u16, y: u16, mask: u8 },
}

/// Apply queued input, in order.
///
/// Order is preserved because it is the difference between typing and not: two
/// `sendkey`s arriving the other way round produce a different character.
async fn input_task(
    injector: Arc<QmpInputInjector>,
    mut commands: tokio::sync::mpsc::Receiver<InputCommand>,
    peer: SocketAddr,
) {
    // Pointer button state, so a mouse *move* does not re-send every button.
    let mut button_mask = 0u8;
    while let Some(command) = commands.recv().await {
        match command {
            InputCommand::Key { keysym, down } => apply_key(&injector, keysym, down).await,
            InputCommand::Pointer { x, y, mask } => {
                apply_pointer(&injector, &mut button_mask, x, y, mask).await;
            }
        }
        tracing::trace!(%peer, "input applied");
    }
}

/// Negotiate version, security type and credentials.
///
/// Nothing about the guest is revealed before this succeeds: no desktop size, no
/// pixels, and no input is applied on this connection's behalf.
async fn handshake(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    shared: &Arc<Shared>,
) -> Result<(), RfbError> {
    let mut line = [0u8; MAX_VERSION_BANNER_BYTES];
    let mut version = 0u16;
    for _ in 0..MAX_VERSION_RETRIES {
        writer.write_all(SERVER_VERSION_BANNER).await?;
        writer.flush().await?;

        reader.read_exact(&mut line).await?;
        match decide_version(parse_client_version(&line)?)? {
            VersionDecision::Accept(agreed) => {
                version = agreed;
                break;
            }
            // The client asked for something newer than 3.8. Re-offer and let it
            // come back with something this server can actually speak.
            VersionDecision::Retry => continue,
        }
    }
    if version == 0 {
        return Err(RfbError::Protocol(
            "client insisted on a protocol version above 3.8".into(),
        ));
    }

    match choose_security(reader, writer, shared, version).await? {
        SecurityType(SECURITY_VNC_AUTH) => {
            let password = VncPassword::new(&shared.config.password)?;
            verify_vnc_auth(reader, writer, &password, version).await?;
        }
        SecurityType(SECURITY_NONE) => {}
        SecurityType(other) => {
            return Err(RfbError::Protocol(format!(
                "security type {other} was negotiated but is not implemented"
            )))
        }
    }

    // ClientInit: one byte, non-zero meaning "share the desktop with others".
    let mut client_init = [0u8; 1];
    reader.read_exact(&mut client_init).await?;
    if client_init[0] != 0 {
        // Accepted rather than refused: it is still an authenticated client, and
        // every client counts against max_clients regardless of sharing.
        tracing::debug!("client requested a shared desktop");
    }
    Ok(())
}

/// Agree a security type.
///
/// The 3.7/3.8 shape is asymmetric and easy to get backwards: the **server**
/// sends the list of types it supports, and the **client** replies with a single
/// byte naming the one it wants. Only 3.3 has the server state its choice, as a
/// `u32`, with no list at all.
async fn choose_security(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    shared: &Arc<Shared>,
    version: u16,
) -> Result<SecurityType, RfbError> {
    let offer = security_type_offer(shared.config.allow_none_auth);

    if version == 303 {
        // 3.3 has no list exchange: the server decides and says so.
        let chosen = if offer.contains(&SECURITY_VNC_AUTH) {
            SECURITY_VNC_AUTH
        } else {
            offer[0]
        };
        writer.write_all(&u32::from(chosen).to_be_bytes()).await?;
        writer.flush().await?;
        return Ok(SecurityType(chosen));
    }

    let mut list = Vec::with_capacity(offer.len());
    list.push(offer.len() as u8);
    list.extend_from_slice(&offer);
    writer.write_all(&list).await?;
    writer.flush().await?;

    // One byte back: the client's choice.
    let mut choice = [0u8; 1];
    reader.read_exact(&mut choice).await?;
    let choice = choice[0];

    if !offer.contains(&choice) {
        return Err(RfbError::Protocol(format!(
            "the client chose security type {choice}, which this server did not offer; \
             this server offers {:?}",
            offer
        )));
    }
    // The policy lives in one place: a client may not talk this server down into
    // `None` when only VNC Auth was offered, whatever it asked for.
    let selected = negotiate_security_type(&[choice], shared.config.allow_none_auth)?;
    Ok(selected)
}

/// The VNC Authentication exchange, then `SecurityResult`.
async fn verify_vnc_auth(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    password: &VncPassword,
    version: u16,
) -> Result<(), RfbError> {
    let challenge = generate_challenge();
    writer.write_all(&challenge).await?;
    writer.flush().await?;

    let mut response = [0u8; RESPONSE_LEN];
    reader.read_exact(&mut response).await?;

    let ok = password.verify(&challenge, &response);
    // Only 3.8 carries a reason string; 3.3 and 3.7 close without one.
    let reason = (version == 308).then_some("authentication failed");
    let mut result = Vec::with_capacity(16);
    write_security_result(&mut result, ok, reason);
    writer.write_all(&result).await?;
    writer.flush().await?;

    if !ok {
        // Deliberately says nothing about which half was wrong, how close the
        // response was, or whether a password is even configured.
        return Err(RfbError::AuthFailed);
    }
    Ok(())
}

/// Derive the framebuffer geometry the capture backend will produce, enforcing
/// every bound before a single byte of it is allocated.
fn framebuffer_geometry(capture: &CaptureSource) -> Result<(u16, u16), RfbError> {
    let (width, height) = capture.geometry();
    let too_large = || RfbError::FramebufferTooLarge {
        width,
        height,
        max_pixels: MAX_FRAMEBUFFER_PIXELS,
        max_dimension: MAX_FRAMEBUFFER_DIMENSION,
    };
    if width == 0 || height == 0 {
        return Err(too_large());
    }
    if width > MAX_FRAMEBUFFER_DIMENSION || height > MAX_FRAMEBUFFER_DIMENSION {
        return Err(too_large());
    }
    if width as u64 * height as u64 > u64::from(MAX_FRAMEBUFFER_PIXELS) {
        return Err(too_large());
    }
    Ok((width as u16, height as u16))
}

/// The per-client steady state: read requests, react to frames, queue input.
async fn session_loop(
    state: &mut ClientState,
    out_tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
    mut events: tokio::sync::mpsc::Receiver<ClientEvent>,
    input_tx: &tokio::sync::mpsc::Sender<InputCommand>,
    mut frames: broadcast::Receiver<Arc<Framebuffer>>,
    shared: &Arc<Shared>,
    peer: SocketAddr,
) -> Result<(), RfbError> {
    let idle = shared.config.idle_timeout;
    // `None` parks the timeout branch forever, which is the documented way to
    // disable the idle check without a second code path. Rebuilt after each
    // client message so a fresh deadline is picked up.
    let mut deadline: Option<tokio::time::Instant> =
        (!idle.is_zero()).then(|| tokio::time::Instant::now() + idle);

    loop {
        tokio::select! {
            biased;

            event = events.recv() => {
                match event {
                    Some(ClientEvent::Message(message)) => {
                        deadline = (!idle.is_zero())
                            .then(|| tokio::time::Instant::now() + idle);
                        handle_client_message(message, state, input_tx, shared, peer);
                    }
                    Some(ClientEvent::Closed) => return Ok(()),
                    Some(ClientEvent::Failed(reason)) => {
                        return Err(RfbError::Protocol(reason))
                    }
                    None => return Ok(()),
                }
            }

            frame = frames.recv() => {
                match frame {
                    Ok(frame) => {
                        if let Some(update) = build_update(state, &frame) {
                            // `try_send`, never `send`: a full queue means this
                            // client is slower than the desktop. Dropping the frame
                            // costs it one resynchronisation; blocking here would
                            // stall the capture loop for everyone.
                            if out_tx.try_send(update).is_err() {
                                state.needs_full_refresh.store(true, Ordering::Release);
                                tracing::debug!(%peer, "dropped a frame for a slow client");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        // The client missed frames entirely, so its view is no
                        // longer the frame it was last sent and `CopyRect` is off
                        // the table until it has been caught up in full.
                        state.needs_full_refresh.store(true, Ordering::Release);
                        tracing::debug!(%peer, missed, "client fell behind the capture loop");
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }

            _ = async move {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                return Err(RfbError::Protocol(format!(
                    "no traffic for {}s; disconnecting",
                    idle.as_secs()
                )))
            }
        }
    }
}

/// Total length of the client message starting at `head`, if it can be determined.
///
/// Returns `Ok(None)` when `head` is too short to tell, which is the normal case
/// for a message split across TCP segments. A message type this server does not
/// implement is an error rather than a skip: an unknown framing cannot be skipped
/// without losing stream synchronisation, so the connection has to end.
fn framed_length(head: &[u8]) -> Result<Option<usize>, RfbError> {
    let Some(&message_type) = head.first() else {
        return Ok(None);
    };
    let fixed = match message_type {
        // SetPixelFormat: 1 + 3 padding + 16.
        0 => Some(20),
        // FramebufferUpdateRequest.
        3 => Some(10),
        // KeyEvent.
        4 => Some(8),
        // PointerEvent.
        5 => Some(6),
        _ => None,
    };
    if let Some(length) = fixed {
        return Ok((head.len() >= length).then_some(length));
    }
    match message_type {
        2 => {
            // SetEncodings: 1 + 1 padding + 2 count, then 4 per encoding.
            if head.len() < 4 {
                return Ok(None);
            }
            let count = u16::from_be_bytes([head[2], head[3]]) as usize;
            if count > MAX_CLIENT_ENCODINGS_LIMIT {
                return Err(RfbError::Undecodable(format!(
                    "client advertised {count} encodings; this server accepts at most \
                     {MAX_CLIENT_ENCODINGS_LIMIT}"
                )));
            }
            Ok((head.len() >= 4 + count * 4).then_some(4 + count * 4))
        }
        6 => {
            // ClientCutText: 1 + 3 padding + 4 length, then the text.
            if head.len() < 8 {
                return Ok(None);
            }
            let length =
                u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
            if length > MAX_CUT_TEXT_BYTES {
                return Err(RfbError::Undecodable(format!(
                    "ClientCutText claimed {length} bytes; this server accepts at most \
                     {MAX_CUT_TEXT_BYTES}"
                )));
            }
            Ok((head.len() >= 8 + length).then_some(8 + length))
        }
        other => Err(RfbError::Undecodable(format!(
            "client message type {other} is not implemented; its length is unknown, so \
             the stream cannot be resynchronised"
        ))),
    }
}

/// Largest `SetEncodings` count accepted, checked before allocating for it.
const MAX_CLIENT_ENCODINGS_LIMIT: usize = 1024;

/// Apply one client message.
///
/// Synchronous by design: every side effect that could block — applying input,
/// writing pixels — is handed to a task with its own queue, so this cannot stall
/// the update loop.
fn handle_client_message(
    message: ClientMessage,
    state: &mut ClientState,
    input_tx: &tokio::sync::mpsc::Sender<InputCommand>,
    shared: &Arc<Shared>,
    peer: SocketAddr,
) {
    match message {
        ClientMessage::SetPixelFormat(format) => match format.validate() {
            Ok(()) => {
                // Every pixel already on this client's wire was in the old
                // format, so the view can no longer be compared against a fresh
                // frame and has to be rebuilt from scratch.
                state.format = format;
                state.needs_full_refresh.store(true, Ordering::Release);
                tracing::debug!(%peer, "client changed its pixel format");
            }
            // A format this server cannot produce is ignored rather than fatal:
            // the client keeps working in whatever it last agreed to.
            Err(e) => tracing::debug!(%peer, error = %e, "rejected a pixel format"),
        },

        ClientMessage::SetEncodings { encodings } => {
            state.preferences = EncodingPreferences::new(&encodings);
            let unsupported: Vec<i32> = state
                .preferences
                .advertised()
                .iter()
                .filter(|encoding| !encoding.is_producible())
                .map(|encoding| encoding.code())
                .collect();
            if !unsupported.is_empty() {
                // Perfectly normal: every real client advertises a dozen
                // pseudo-encodings. They are simply never sent.
                tracing::debug!(
                    %peer,
                    ?unsupported,
                    "encodings this server will never send"
                );
            }
        }

        ClientMessage::FramebufferUpdateRequest {
            incremental,
            x,
            y,
            width,
            height,
        } => {
            if !incremental {
                state.needs_full_refresh.store(true, Ordering::Release);
            }
            // A new request supersedes an outstanding one: it is at least as
            // fresh, and honouring both would send the same damage twice.
            state.pending = Some(PendingRequest {
                x,
                y,
                width,
                height,
            });
        }

        ClientMessage::KeyEvent { down, keysym } => {
            if shared.config.allow_input {
                enqueue_input(
                    input_tx,
                    InputCommand::Key { keysym, down },
                    peer,
                    "key",
                );
            }
        }

        ClientMessage::PointerEvent { x, y, mask } => {
            if shared.config.allow_input {
                enqueue_input(
                    input_tx,
                    InputCommand::Pointer { x, y, mask },
                    peer,
                    "pointer",
                );
            }
        }

        ClientMessage::ClientCutText { text } => {
            // Accepted, logged, never applied. Pasting a peer's clipboard into a
            // real machine needs an owner for that decision.
            tracing::debug!(%peer, bytes = text.len(), "received client cut text (ignored)");
        }

        ClientMessage::Ignored { message_type } => {
            tracing::trace!(%peer, message_type, "ignored an unimplemented client message");
        }
    }
}

/// Queue one input command, dropping it if the queue is full.
///
/// Frames outrank input: a client flooding the guest with pointer moves must
/// never cost it its framebuffer updates, so a full input queue loses the input
/// and says so.
fn enqueue_input(
    input_tx: &tokio::sync::mpsc::Sender<InputCommand>,
    command: InputCommand,
    peer: SocketAddr,
    kind: &'static str,
) {
    if input_tx.try_send(command).is_err() {
        tracing::debug!(
            %peer,
            kind,
            "dropped an input event: the guest is not keeping up with input"
        );
    }
}

/// Turn one captured frame into the bytes to put on the wire, if any.
///
/// `None` means there is nothing to send: no update has been requested, nothing
/// changed, or the damage fell outside the requested window. The second case is
/// the point of the whole module.
fn build_update(state: &mut ClientState, frame: &Framebuffer) -> Option<Vec<u8>> {
    // RFB updates are demand-driven. Nothing goes out until the client asks.
    let request = state.pending.as_ref()?;

    if state.view.width() != frame.width() || state.view.height() != frame.height() {
        // A resolution change invalidates every pixel the client holds. Announce
        // the new size with the pseudo-encoding, and leave the full-refresh flag
        // set so the pixels follow immediately rather than being lost.
        state.view.resize(frame.width(), frame.height());
        state.view.copy_from(frame);
        state.needs_full_refresh.store(true, Ordering::Release);
        let mut out = Vec::new();
        write_framebuffer_update_header(&mut out, 1);
        crate::rfb::encoder::write_desktop_size(&mut out, frame.width(), frame.height());
        return Some(out);
    }

    if state.needs_full_refresh.swap(false, Ordering::AcqRel) {
        let full = full_frame(frame.width(), frame.height());
        state.view.copy_from(frame);
        return assemble(state, frame, vec![full], None);
    }

    let window = request.clip(frame.width(), frame.height())?;
    let (regions, scroll) = plan_regions(state, frame, window);
    if regions.is_empty() && scroll.is_none() {
        return None;
    }

    // Commit the new pixels to what this client now holds. Doing this before the
    // bytes are queued is what makes `CopyRect` sound: the copy source is only
    // ever a pixel this client provably received.
    if let Some(scroll) = scroll {
        // The regions in this branch are exactly `exposed_by_scroll`.
        state.view.scroll(scroll.dx, scroll.dy);
    }
    state.view.apply(frame, &regions);

    assemble(state, frame, regions, scroll)
}

/// Serialise one update message from a list of regions.
fn assemble(
    state: &ClientState,
    frame: &Framebuffer,
    regions: Vec<ChangedRegion>,
    scroll: Option<Scroll>,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    write_framebuffer_update_header(&mut out, 0);
    let mut rectangles = 0u16;

    if let Some(scroll) = scroll {
        rectangles += 1;
        write_copy_rect(
            &mut out,
            &CopyRect {
                dst_x: 0,
                dst_y: 0,
                width: frame.width(),
                height: frame.height(),
                src_x: scroll.dx as u16,
                src_y: scroll.dy as u16,
            },
        );
    }

    for region in &regions {
        rectangles += encode_region(&mut out, state, frame, region, rectangles);
    }

    if rectangles == 0 {
        return None;
    }
    out[2..4].copy_from_slice(&rectangles.to_be_bytes());
    Some(out)
}

/// Encode one region, splitting it if the chosen encoding requires it.
///
/// Returns how many rectangles were written, so the caller can stop at the
/// protocol's per-message limit rather than truncating a stream mid-rectangle.
fn encode_region(
    out: &mut Vec<u8>,
    state: &ClientState,
    frame: &Framebuffer,
    region: &ChangedRegion,
    already: u16,
) -> u16 {
    // Tight's specification caps a rectangle at 2048 pixels wide, so a wide
    // region is split rather than sent whole.
    let pieces = if state.preferences.supports(Encoding::Tight) {
        split_width(region, MAX_TIGHT_RECT_WIDTH)
    } else {
        vec![*region]
    };
    let mut written = 0u16;
    for piece in pieces {
        if already as usize + written as usize >= MAX_RECTS_PER_UPDATE {
            break;
        }
        let encoding = choose_encoding(state, frame, &piece);
        frame.write_rect(out, &piece, encoding, &state.format);
        written += 1;
    }
    written
}

/// Decide what to send for this frame: damage rectangles, a scroll, or nothing.
fn plan_regions(
    state: &ClientState,
    frame: &Framebuffer,
    window: ChangedRegion,
) -> (Vec<ChangedRegion>, Option<Scroll>) {
    let damage = state.view.damage(frame);
    if damage.is_empty() {
        return (Vec::new(), None);
    }

    let frame_pixels = frame.pixel_count();
    let bounding = bounding_region(&damage);
    // A scroll rewrites almost the whole screen, so it is only worth looking for
    // when the damage really is the whole screen and the client asked for an
    // incremental update.
    if damage.len() == 1 && covers_most_of(&bounding, frame_pixels) {
        if let Some(scroll) = state.view.detect_scroll(frame) {
            let exposed = state.view.exposed_by_scroll(scroll.dx, scroll.dy);
            // Worth doing only if the exposed strips are cheaper than the damage
            // they replace, which for a real scroll they are by orders of
            // magnitude.
            if total_area(&exposed) < total_area(&damage) {
                return (exposed, Some(scroll));
            }
        }
    }

    let clipped: Vec<ChangedRegion> = damage
        .into_iter()
        .filter(|region| overlaps(region, &window))
        .map(|region| intersect(&region, &window))
        .filter(|region| region.width > 0 && region.height > 0)
        .collect();
    (clipped, None)
}

/// The smallest rectangle containing every region.
fn bounding_region(regions: &[ChangedRegion]) -> ChangedRegion {
    let Some((first, rest)) = regions.split_first() else {
        return ChangedRegion {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
    };
    let mut bounds = *first;
    for region in rest {
        let right = (u32::from(bounds.x) + u32::from(bounds.width))
            .max(u32::from(region.x) + u32::from(region.width));
        let bottom = (u32::from(bounds.y) + u32::from(bounds.height))
            .max(u32::from(region.y) + u32::from(region.height));
        bounds.x = bounds.x.min(region.x);
        bounds.y = bounds.y.min(region.y);
        bounds.width = (right - u32::from(bounds.x)) as u16;
        bounds.height = (bottom - u32::from(bounds.y)) as u16;
    }
    bounds
}

fn overlaps(a: &ChangedRegion, b: &ChangedRegion) -> bool {
    u32::from(a.x) < u32::from(b.x) + u32::from(b.width)
        && u32::from(b.x) < u32::from(a.x) + u32::from(a.width)
        && u32::from(a.y) < u32::from(b.y) + u32::from(b.height)
        && u32::from(b.y) < u32::from(a.y) + u32::from(a.height)
}

fn intersect(a: &ChangedRegion, b: &ChangedRegion) -> ChangedRegion {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (u32::from(a.x) + u32::from(a.width)).min(u32::from(b.x) + u32::from(b.width));
    let bottom = (u32::from(a.y) + u32::from(a.height)).min(u32::from(b.y) + u32::from(b.height));
    ChangedRegion {
        x,
        y,
        width: right.saturating_sub(u32::from(x)) as u16,
        height: bottom.saturating_sub(u32::from(y)) as u16,
    }
}

/// Choose the encoding for one rectangle, refusing Hextile where it is illegal.
fn choose_encoding(state: &ClientState, frame: &Framebuffer, region: &ChangedRegion) -> Encoding {
    if region.width == 0 || region.height == 0 {
        return Encoding::Raw;
    }
    let solid = frame.is_solid(region);
    let mut encoding = state
        .preferences
        .choose(u64::from(region.width) * u64::from(region.height), solid);
    if encoding == Encoding::Hextile && !hextile_aligned(region, frame.width(), frame.height()) {
        // An unaligned interior Hextile makes a client derive the wrong tile count
        // and lose stream synchronisation for good, so it never goes out.
        encoding = if state.preferences.supports(Encoding::Tight) {
            Encoding::Tight
        } else {
            Encoding::Raw
        };
    }
    encoding
}

/// Translate an RFB key event into QMP `sendkey` and apply it.
///
/// An unmappable keysym is logged rather than guessed at: sending the wrong key
/// into a real guest is worse than sending none, but silently dropping a
/// keystroke makes a broken client impossible to diagnose.
async fn apply_key(injector: &QmpInputInjector, keysym: u32, down: bool) {
    if !down {
        // `sendkey` presses and releases atomically, so a release has nothing to
        // send. A client holding a key across frames gets a discrete tap, which
        // is the best HMP offers.
        tracing::trace!(keysym = format!("{keysym:#x}"), "key release is implicit in sendkey");
        return;
    }
    match keysym_to_sendkey(keysym) {
        Ok(name) => {
            if let Err(e) = injector.send_key_name(&name).await {
                tracing::warn!(error = %e, "key injection failed");
            }
        }
        Err(e) => tracing::debug!(
            keysym = format!("{keysym:#x}"),
            error = %e,
            "no HMP key for this keysym; the keystroke was not injected"
        ),
    }
}

/// Translate an RFB pointer event into QMP `input-send-event`.
///
/// Only *changes* are sent. A mouse move is the most frequent event RFB produces
/// and each one is a QMP round trip on a socket the capture loop also contends
/// for, so re-stating every button on every move would halve the achievable
/// pointer rate for nothing.
async fn apply_pointer(
    injector: &QmpInputInjector,
    button_mask: &mut u8,
    x: u16,
    y: u16,
    mask: u8,
) {
    if let Err(e) = injector.move_pointer(x as i32, y as i32).await {
        tracing::warn!(error = %e, "pointer move failed");
        return;
    }

    let changed = *button_mask ^ mask;
    for (bit, button) in [
        (1u8, MouseButton::Left),
        (2, MouseButton::Middle),
        (4, MouseButton::Right),
    ] {
        if changed & bit == 0 {
            continue;
        }
        if let Err(e) = injector.button(button, mask & bit != 0).await {
            tracing::warn!(error = %e, "pointer button failed");
        }
    }

    // RFB puts the wheel in bits 3-6 with the bit position encoding direction.
    let wheel = mask & 0b0111_1000;
    if wheel != 0 {
        let dy = if wheel & 0b0001_0000 != 0 {
            1
        } else if wheel & 0b0010_0000 != 0 {
            -1
        } else {
            0
        };
        let dx = if wheel & 0b0100_0000 != 0 {
            1
        } else if wheel & 0b1000_0000 != 0 {
            -1
        } else {
            0
        };
        if let Err(e) = injector.scroll(dx, dy).await {
            tracing::warn!(error = %e, "pointer scroll failed");
        }
    }
    *button_mask = mask;
}

/// Map an X11 keysym onto the HMP `sendkey` vocabulary in [`crate::input_qmp`].
///
/// Latin-1 keysyms are printable characters and go through [`key_for`]. The
/// function keysyms are mapped by name through [`control_key`]. Everything else is
/// an error: the alternative is inventing a key name, and a wrong key injected
/// into a guest at 50 ms intervals is an effective way to corrupt something.
pub fn keysym_to_sendkey(keysym: u32) -> Result<String, UnsupportedKeysym> {
    // X11 reports a held Ctrl as a control character rather than as "the C key
    // with Ctrl down": Ctrl+C arrives as keysym 0x03. Map that range back onto
    // ctrl-<letter> so it types into the guest instead of being rejected.
    // Without this, Ctrl+C / Ctrl+V / Ctrl+D -- the three keystrokes a
    // terminal or editor needs most -- cannot be delivered at all, and the
    // only symptom is a log line.
    if (0x01..=0x1A).contains(&keysym) {
        let letter = char::from_u32(0x61u32 + (keysym - 0x01)).ok_or(UnsupportedKeysym)?;
        let mut mods = ModifierKeys::default();
        mods.ctrl = true;
        return resolve_sendkey(letter.to_string().as_str(), mods).map_err(|_| UnsupportedKeysym);
    }

    // Alt is reported as 0x0100 + the base character (Alt+a -> 0x0161).
    if (0x0100..=0x017F).contains(&keysym) {
        let base = keysym - 0x0100;
        if (0x20..=0x7E).contains(&base) {
            let ch = char::from_u32(base).ok_or(UnsupportedKeysym)?;
            let mut mods = ModifierKeys::default();
            mods.alt = true;
            return resolve_sendkey(ch.to_string().as_str(), mods).map_err(|_| UnsupportedKeysym);
        }
    }

    // Latin-1: the keysym *is* the Unicode code point.
    if (0x20..=0x7E).contains(&keysym) || (0xA0..=0xFF).contains(&keysym) {
        let ch = char::from_u32(keysym).ok_or(UnsupportedKeysym)?;
        return key_for(ch)
            .map(str::to_string)
            .map_err(|_| UnsupportedKeysym);
    }
    if let Some(name) = keysym_name(keysym) {
        return control_key(name).map(str::to_string).ok_or(UnsupportedKeysym);
    }
    Err(UnsupportedKeysym)
}

/// A keysym with no mapping into the existing HMP key vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no HMP sendkey name for this keysym")]
pub struct UnsupportedKeysym;

/// Non-printing keysyms and their RFB names.
///
/// An explicit table rather than a range calculation, so that a keysym this server
/// does not know about fails loudly instead of being turned into a
/// plausible-looking key name.
///
/// Function keys F1-F12 are deliberately absent: `control_key` does not know
/// them, so mapping them here would produce a name that resolves to nothing.
/// They are rejected and logged until `input_qmp`'s table grows, which keeps the
/// rejection honest.
const NAMED_KEYSYMS: &[(u32, &str)] = &[
    (0xFF08, "backspace"),
    (0xFF09, "tab"),
    (0xFF0D, "ret"),
    (0xFF1B, "esc"),
    (0xFF50, "home"),
    (0xFF51, "left"),
    (0xFF52, "up"),
    (0xFF53, "right"),
    (0xFF54, "down"),
    (0xFF57, "end"),
    (0xFF63, "insert"),
    (0xFFFF, "delete"),
];

/// The RFB name for a keysym, if this server has one.
fn keysym_name(keysym: u32) -> Option<&'static str> {
    NAMED_KEYSYMS
        .iter()
        .find(|(value, _)| *value == keysym)
        .map(|(_, name)| *name)
}

/// Build the `continuum-core` input event for a keysym, for callers that want to
/// batch or inspect input rather than apply it immediately.
pub fn keysym_to_input_event(keysym: u32, down: bool) -> Result<RemoteInputEvent, UnsupportedKeysym> {
    let name = keysym_to_sendkey(keysym)?;
    Ok(RemoteInputEvent {
        action: if down {
            InputAction::KeyPress
        } else {
            InputAction::KeyUp
        },
        x: None,
        y: None,
        button: None,
        key: Some(name),
        modifiers: Some(ModifierKeys::default()),
        scroll_x: None,
        scroll_y: None,
        monitor_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfb::encoder::all_contained;
    use crate::rfb::proto::{PixelFormat as Fmt, MAX_CLIENT_ENCODINGS};

    // ── Configuration ─────────────────────────────────────────────────────

    #[test]
    fn defaults_are_loopback_and_authenticated() {
        let config = RfbServerConfig::secure(format!("127.0.0.1:{DEFAULT_PORT}").parse().unwrap(), "hunter2");
        assert!(config.bind.ip().is_loopback(), "must not default to a routable address");
        assert!(!config.allow_none_auth, "VNC Auth must be on by default");
        assert_eq!(config.max_clients, DEFAULT_MAX_CLIENTS);
        assert_eq!(config.write_queue_depth, DEFAULT_WRITE_QUEUE_DEPTH);
        assert_eq!(config.pixel_format, Fmt::BGRX32);
    }

    #[test]
    fn configuration_debug_output_never_contains_the_password() {
        let config = RfbServerConfig::secure("127.0.0.1:5900".parse().unwrap(), "sesame");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sesame"), "the password leaked into Debug: {rendered}");
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn configuration_validation_rejects_unusable_settings() {
        let good = RfbServerConfig::secure("127.0.0.1:5900".parse().unwrap(), "sesame");
        assert!(good.validate().is_ok());

        assert!(
            RfbServerConfig::secure("127.0.0.1:5900".parse().unwrap(), "")
                .validate()
                .is_err(),
            "an empty password authenticates nobody"
        );
        assert!(
            RfbServerConfig::secure("127.0.0.1:5900".parse().unwrap(), "123456789")
                .validate()
                .is_err(),
            "a ninth character would be silently ignored by RFC 6143"
        );

        let mut no_clients = good.clone();
        no_clients.max_clients = 0;
        assert!(no_clients.validate().is_err());

        let mut no_queue = good.clone();
        no_queue.write_queue_depth = 0;
        assert!(no_queue.validate().is_err());

        let mut palette = good.clone();
        palette.pixel_format.true_colour = false;
        assert!(palette.validate().is_err());

        // Exactly eight bytes is the boundary and must be accepted.
        assert!(RfbServerConfig::secure("127.0.0.1:5900".parse().unwrap(), "12345678")
            .validate()
            .is_ok());
    }

    #[test]
    fn framebuffer_bounds_are_fixed() {
        // The caps are what stop a guest advertising an absurd resolution from
        // turning into an absurd allocation: at the default 32bpp, 4.2M pixels
        // is 16 MiB per client and 4K still fits.
        const _: () = assert!(MAX_FRAMEBUFFER_PIXELS <= 8 * 1024 * 1024);
        const _: () = assert!(MAX_FRAMEBUFFER_DIMENSION >= 3840, "4K must fit");
        assert_eq!(clamp_dimension(0), 1);
        assert_eq!(clamp_dimension(100_000), 8192);
    }

    #[test]
    fn client_slots_are_bounded_and_released() {
        let slots = AdmissionSlots::new(2);
        assert!(slots.claim());
        assert!(slots.claim());
        assert!(!slots.claim(), "a third client must be refused, not queued");
        assert_eq!(slots.in_use(), 2);

        slots.release();
        assert_eq!(slots.in_use(), 1);
        assert!(slots.claim(), "releasing a slot must let someone in");
        assert_eq!(slots.in_use(), 2);

        // Exhausting every slot and releasing them all returns to zero, so a
        // long-lived listener does not leak slots across client churn.
        for _ in 0..8 {
            if slots.claim() {
                slots.release();
            }
        }
        assert_eq!(slots.in_use(), 2);
        slots.release();
        slots.release();
        assert_eq!(slots.in_use(), 0);
    }

    #[test]
    fn zero_is_never_a_valid_client_limit() {
        let slots = AdmissionSlots::new(0);
        assert!(!slots.claim(), "a server with no slots admits nobody");
    }

    // ── Input ─────────────────────────────────────────────────────────────

    #[test]
    fn printable_keysyms_map_through_the_existing_key_vocabulary() {
        // The mapping must reuse `input_qmp::key_for` rather than invent names.
        for (keysym, expected) in [
            (0x61u32, "a"),
            (0x41, "shift-a"),
            (0x31, "1"),
            (0x21, "shift-1"),
            (0x20, "spc"),
            (0x2D, "minus"),
            (0x7E, "shift-grave_accent"),
        ] {
            assert_eq!(keysym_to_sendkey(keysym).unwrap(), expected, "keysym {keysym:#x}");
        }
    }

    #[test]
    fn control_keysyms_map_by_name() {
        for (keysym, expected) in [
            (0xFF0Du32, "ret"),
            (0xFF09, "tab"),
            (0xFF1B, "esc"),
            (0xFF08, "backspace"),
            (0xFF50, "home"),
            (0xFF54, "down"),
            (0xFFFF, "delete"),
        ] {
            let mapped = keysym_to_sendkey(keysym).unwrap();
            assert!(mapped.contains(expected), "keysym {keysym:#x} produced {mapped:?}");
        }
    }

    #[test]
    fn an_unknown_keysym_is_rejected_rather_than_guessed() {
        // Never silently dropped, never turned into a plausible-looking key.
        for keysym in [0x0000u32, 0x0100_0001, 0xFF9B, 0xFE50, 0x1234_5678] {
            assert!(
                keysym_to_sendkey(keysym).is_err(),
                "keysym {keysym:#x} should have been rejected"
            );
        }
    }

    #[test]
    fn function_keys_are_rejected_rather_than_mapped_to_a_bare_name() {
        // `control_key` does not know F1, so naming it here would produce a name
        // that resolves to nothing.
        assert_eq!(keysym_name(0xFFBE), None);
        assert!(keysym_to_sendkey(0xFFBE).is_err());
    }

    #[test]
    fn keysym_to_input_event_produces_a_press_or_a_release() {
        let down = keysym_to_input_event(0x61, true).unwrap();
        assert_eq!(down.action, InputAction::KeyPress);
        assert_eq!(down.key.as_deref(), Some("a"));
        assert_eq!(keysym_to_input_event(0x61, false).unwrap().action, InputAction::KeyUp);
        assert!(keysym_to_input_event(0xFE50, true).is_err());
    }

    // ── Message framing ───────────────────────────────────────────────────

    #[test]
    fn framing_finds_the_length_of_every_supported_message() {
        assert_eq!(framed_length(&[0; 20]).unwrap(), Some(20), "SetPixelFormat");
        assert_eq!(framed_length(&[3; 10]).unwrap(), Some(10), "UpdateRequest");
        assert_eq!(framed_length(&[4; 8]).unwrap(), Some(8), "KeyEvent");
        assert_eq!(framed_length(&[5; 6]).unwrap(), Some(6), "PointerEvent");
        assert_eq!(framed_length(&[2, 0, 0, 1, 0, 0, 0, 0]).unwrap(), Some(8), "SetEncodings");
        assert_eq!(framed_length(&[6, 0, 0, 0, 0, 0, 0, 2, 0, 0]).unwrap(), Some(10), "CutText");
    }

    #[test]
    fn framing_asks_for_more_bytes_rather_than_guessing() {
        assert_eq!(framed_length(&[]).unwrap(), None);
        assert_eq!(framed_length(&[0, 0, 0]).unwrap(), None, "SetPixelFormat is incomplete");
        assert_eq!(framed_length(&[2, 0, 0]).unwrap(), None, "the count is missing");
        assert_eq!(framed_length(&[6, 0, 0, 0]).unwrap(), None, "the length is missing");
    }

    #[test]
    fn framing_refuses_unbounded_client_controlled_allocations() {
        // Both lengths are checked before a buffer of that size would exist.
        let encodings = vec![2u8, 0, 0xff, 0xff];
        assert!(framed_length(&encodings).is_err(), "65535 encodings is 256 KiB");

        let cut_text = vec![6u8, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        assert!(framed_length(&cut_text).is_err(), "a 4 GiB clipboard is not a buffer");

        let many = [2u8, 0, 0x04, 0x01];
        assert!(
            framed_length(&many).is_err(),
            "more encodings than the cap must not allocate"
        );
        assert_eq!(MAX_CLIENT_ENCODINGS_LIMIT, MAX_CLIENT_ENCODINGS);
    }

    #[test]
    fn an_unknown_message_type_cannot_be_skipped_and_is_refused() {
        // Its length is unknown, so ignoring it would desynchronise the stream.
        let err = framed_length(&[99, 0, 0, 0]).unwrap_err().to_string();
        assert!(err.contains("resynchronised"), "got: {err}");
    }

    // ── The update loop ───────────────────────────────────────────────────

    #[test]
    fn nothing_is_sent_before_the_client_asks() {
        // RFB updates are demand-driven: a client that has not sent a
        // FramebufferUpdateRequest gets nothing, however much has changed.
        let mut state = test_state(64, 48);
        let frame = solid_frame(64, 48);
        assert!(build_update(&mut state, &frame).is_none());

        request_all(&mut state, 64, 48);
        let first = build_update(&mut state, &frame).expect("the first update is the whole frame");
        assert_eq!(&first[..4], &[0, 0, 0, 1], "one rectangle in one update");
    }

    #[test]
    fn a_static_desktop_produces_no_bytes_at_all() {
        let mut state = test_state(64, 48);
        let frame = solid_frame(64, 48);
        request_all(&mut state, 64, 48);
        let first = build_update(&mut state, &frame).expect("first update");
        assert!(!first.is_empty());

        // Every later poll with the same frame and an outstanding request must
        // produce nothing. This is the whole bandwidth claim.
        for _ in 0..10 {
            request_all(&mut state, 64, 48);
            assert!(
                build_update(&mut state, &frame).is_none(),
                "an unchanged frame must not produce an update"
            );
        }
    }

    #[test]
    fn an_incremental_update_after_a_change_is_small() {
        let mut state = test_state(320, 240);
        let mut frame = solid_frame(320, 240);
        request_all(&mut state, 320, 240);
        build_update(&mut state, &frame).expect("first update");

        // Move a 16x16 block.
        paint(&mut frame, 200, 100, 16, 16, [255, 255, 255, 255]);
        request_all(&mut state, 320, 240);
        let update = build_update(&mut state, &frame).expect("an update");

        assert_eq!(&update[..2], &[0, 0], "message type 0 and its padding");
        assert_eq!(u16::from_be_bytes([update[2], update[3]]), 1, "one rectangle");
        assert_eq!(u16::from_be_bytes([update[4], update[5]]), 200, "rect x");
        assert_eq!(u16::from_be_bytes([update[6], update[7]]), 100, "rect y");
        assert_eq!(u16::from_be_bytes([update[8], update[9]]), 16, "rect width");
        assert_eq!(u16::from_be_bytes([update[10], update[11]]), 16, "rect height");

        let full_frame_bytes = 320 * 240 * 4;
        assert!(
            update.len() * 20 < full_frame_bytes,
            "a 16x16 change produced {} bytes against a {full_frame_bytes}-byte full frame",
            update.len()
        );
        assert_eq!(state.view, frame, "the client's view must be resynchronised");
    }

    #[test]
    fn a_dropped_frame_forces_a_resynchronisation() {
        // The invariant that keeps `CopyRect` sound: a client that missed an
        // update is told to forget what it thinks it has.
        let mut state = test_state(64, 48);
        let frame = solid_frame(64, 48);
        request_all(&mut state, 64, 48);
        build_update(&mut state, &frame).unwrap();

        state.needs_full_refresh.store(true, Ordering::Release);
        request_all(&mut state, 64, 48);
        let resync = build_update(&mut state, &frame).expect("resync");
        assert_eq!(u16::from_be_bytes([resync[2], resync[3]]), 1);
        assert!(!state.needs_full_refresh.load(Ordering::Acquire));
    }

    #[test]
    fn a_non_incremental_request_resynchronises() {
        let mut state = test_state(64, 48);
        let frame = solid_frame(64, 48);
        request_all(&mut state, 64, 48);
        build_update(&mut state, &frame).unwrap();

        state.pending = Some(PendingRequest {
            x: 0,
            y: 0,
            width: 64,
            height: 48,
        });
        state.needs_full_refresh.store(true, Ordering::Release);
        let full = build_update(&mut state, &frame).expect("full frame");
        assert_eq!(u16::from_be_bytes([full[8], full[9]]), 64, "the whole width");
    }

    #[test]
    fn damage_outside_the_requested_window_is_not_sent() {
        let mut state = test_state(64, 48);
        let mut frame = solid_frame(64, 48);
        request_all(&mut state, 64, 48);
        build_update(&mut state, &frame).unwrap();

        paint(&mut frame, 0, 0, 4, 4, [1, 2, 3, 255]);
        // Ask only about the bottom-right quadrant.
        state.pending = Some(PendingRequest {
            x: 32,
            y: 24,
            width: 32,
            height: 24,
        });
        assert!(
            build_update(&mut state, &frame).is_none(),
            "damage outside the requested window must not be sent"
        );
        // And it is still pending, so a later full-frame request picks it up.
        request_all(&mut state, 64, 48);
        assert!(build_update(&mut state, &frame).is_some());
    }

    #[test]
    fn a_scrolled_frame_becomes_a_copyrect_plus_the_exposed_strip() {
        // Horizontal bands: a vertical scroll moves every pixel, so the damage is
        // exactly one full-frame rectangle and the scroll heuristic applies. A
        // frame with per-pixel texture would produce many small rectangles
        // instead, which is correct behaviour and covered elsewhere.
        let mut state = test_state(64, 48);
        let previous = banded_frame(64, 48);
        state.view.copy_from(&previous);
        state.needs_full_refresh.store(false, Ordering::Release);
        request_all(&mut state, 64, 48);
        assert!(
            build_update(&mut state, &previous).is_none(),
            "nothing changed yet"
        );

        let scrolled = scrolled_frame(&previous, 0, 5);
        let update = build_update(&mut state, &scrolled).expect("a scroll update");

        assert_eq!(&update[..2], &[0, 0]);
        assert_eq!(
            u16::from_be_bytes([update[2], update[3]]),
            2,
            "a CopyRect plus the exposed strip"
        );
        assert_eq!(
            i32::from_be_bytes(update[12..16].try_into().unwrap()),
            1,
            "the first rectangle is CopyRect"
        );
        assert_eq!(&update[16..20], &[0, 0, 0, 5], "copying source (0, 5)");
        assert_eq!(state.view, scrolled, "the client's view must be resynchronised");
        assert!(
            update.len() < 4096,
            "a scroll must be cheap, not a full frame"
        );
    }

    #[test]
    fn a_resolution_change_is_announced_with_desktopsize() {
        let mut state = test_state(64, 48);
        request_all(&mut state, 64, 48);
        build_update(&mut state, &solid_frame(64, 48)).unwrap();

        let bigger = solid_frame(80, 60);
        request_all(&mut state, 80, 60);
        let update = build_update(&mut state, &bigger).expect("a resize update");
        assert_eq!(&update[..4], &[0, 0, 0, 1]);
        assert_eq!(u16::from_be_bytes([update[4], update[5]]), 0, "x is zero");
        assert_eq!(u16::from_be_bytes([update[6], update[7]]), 0, "y is zero");
        assert_eq!(u16::from_be_bytes([update[8], update[9]]), 80, "the new width");
        assert_eq!(u16::from_be_bytes([update[10], update[11]]), 60, "the new height");
        assert_eq!(
            i32::from_be_bytes(update[12..16].try_into().unwrap()),
            -223,
            "the DesktopSize pseudo-encoding"
        );
        assert_eq!((state.view.width(), state.view.height()), (80, 60));

        // And the next update resynchronises in full, because a resize
        // invalidates every pixel the client held.
        request_all(&mut state, 80, 60);
        state.needs_full_refresh.store(true, Ordering::Release);
        let resync = build_update(&mut state, &bigger).expect("resync");
        assert_eq!(u16::from_be_bytes([resync[8], resync[9]]), 80);
    }

    #[test]
    fn a_client_asking_for_five_five_five_gets_five_five_five_bytes() {
        let mut state = test_state(32, 32);
        state.format = Fmt::RGB555;
        request_all(&mut state, 32, 32);
        let update = build_update(&mut state, &solid_frame(32, 32)).expect("first update");
        // 4 bytes of message header, 12 of rectangle header, then the pixels.
        assert_eq!(update.len(), 16 + 32 * 32 * 2, "16bpp is half of 32bpp");
    }

    #[test]
    fn a_wide_full_refresh_is_split_so_no_tight_rectangle_exceeds_2048() {
        let mut state = test_state(3000, 8);
        state.preferences = EncodingPreferences::new(&[0, 7]);
        request_all(&mut state, 3000, 8);
        let update = build_update(&mut state, &solid_frame(3000, 8)).expect("first update");

        let count = u16::from_be_bytes([update[2], update[3]]);
        assert_eq!(count, 2, "3000 pixels wide is two Tight rectangles");
        // First rectangle: header at 4..16, then the Tight control byte.
        let first_width = u16::from_be_bytes([update[8], update[9]]);
        assert_eq!(first_width, 2048);
        assert_eq!(update[16], 0x01, "the Tight control byte");
        let second_header = 4 + 12 + tight_payload_size(&update[16..]);
        let second_x = u16::from_be_bytes([update[second_header], update[second_header + 1]]);
        let second_width = u16::from_be_bytes([update[second_header + 4], update[second_header + 5]]);
        assert_eq!(second_x, 2048, "the second rectangle starts where the first ended");
        assert_eq!(second_width, 3000 - 2048);
        assert_eq!(
            update.len(),
            second_header + 12 + tight_payload_size(&update[second_header + 12..]),
            "both rectangles were walked to the end of the message"
        );
    }

    #[test]
    fn hextile_is_refused_for_an_unaligned_interior_rectangle() {
        let frame = solid_frame(1280, 800);
        let state = test_state(1280, 800);
        let mut hextile_state = state.clone();
        hextile_state.preferences = EncodingPreferences::new(&[0, 5]);
        let mut tight_state = state.clone();
        tight_state.preferences = EncodingPreferences::new(&[0, 5, 7]);

        let aligned = ChangedRegion {
            x: 4,
            y: 4,
            width: 32,
            height: 32,
        };
        // The specification constrains the rectangle's width and height, not its
        // origin: tiles are cut relative to the rectangle.
        assert!(hextile_aligned(&aligned, 1280, 800));
        assert_eq!(choose_encoding(&hextile_state, &frame, &aligned), Encoding::Hextile);

        // 30 is not a multiple of 16 and the rectangle is nowhere near the edge.
        let unaligned = ChangedRegion {
            x: 4,
            y: 4,
            width: 30,
            height: 32,
        };
        assert!(
            !hextile_aligned(&unaligned, 1280, 800),
            "precondition of the test itself"
        );
        assert_eq!(
            choose_encoding(&hextile_state, &frame, &unaligned),
            Encoding::Raw,
            "an unaligned interior rect must not go out as Hextile"
        );
        assert_eq!(
            choose_encoding(&tight_state, &frame, &unaligned),
            Encoding::Tight,
            "with Tight available it is the fallback"
        );
        assert_eq!(choose_encoding(&state, &frame, &unaligned), Encoding::Raw);

        // Clipped by the framebuffer edge, which the specification permits.
        let clipped = ChangedRegion {
            x: 1262,
            y: 782,
            width: 18,
            height: 18,
        };
        assert!(hextile_aligned(&clipped, 1280, 800));
        assert_eq!(
            choose_encoding(&hextile_state, &frame, &clipped),
            Encoding::Hextile
        );
    }

    #[test]
    fn a_solid_region_goes_out_as_rre_when_the_client_asked_for_it() {
        let mut state = test_state(64, 48);
        state.preferences = EncodingPreferences::new(&[0, 2, 5, 7]);
        request_all(&mut state, 64, 48);
        let update = build_update(&mut state, &solid_frame(64, 48)).expect("first update");
        // Message header 4 bytes, rectangle header 12 bytes, encoding last.
        assert_eq!(
            i32::from_be_bytes(update[12..16].try_into().unwrap()),
            2,
            "a solid 64x48 frame is cheaper as RRE"
        );
        assert_eq!(update.len(), 16 + 8, "four bytes of count and one background pixel");
    }

    // ── Geometry helpers ──────────────────────────────────────────────────

    #[test]
    fn a_request_reaching_past_the_framebuffer_is_clipped_to_it() {
        // A client that has not processed a DesktopSize asks for more than exists;
        // the right answer is the frame it can actually use.
        let oversized = PendingRequest {
            x: 0,
            y: 0,
            width: 4096,
            height: 4096,
        };
        assert_eq!(oversized.clip(1280, 800), Some(full_frame(1280, 800)));

        let window = PendingRequest {
            x: 100,
            y: 100,
            width: 200,
            height: 200,
        };
        let clipped = window.clip(1280, 800).unwrap();
        assert_eq!(clipped.bounds(), (100, 100, 200, 200));
        assert!(all_contained(&[clipped], &full_frame(1280, 800)));

        // A request that ends exactly on the framebuffer edge is legitimate, not an
        // oversized one.
        let exact = PendingRequest {
            x: 32,
            y: 24,
            width: 32,
            height: 24,
        };
        assert_eq!(
            exact.clip(64, 48),
            Some(ChangedRegion {
                x: 32,
                y: 24,
                width: 32,
                height: 24
            })
        );

        assert!(
            PendingRequest { x: 0, y: 0, width: 0, height: 0 }
                .clip(1280, 800)
                .is_none(),
            "a zero-area request must not be answered with the whole frame"
        );
        assert!(
            PendingRequest { x: 2000, y: 0, width: 10, height: 10 }
                .clip(1280, 800)
                .is_none(),
            "a request entirely off-screen has nothing to answer"
        );
    }

    #[test]
    fn bounding_and_intersection_are_conservative() {
        let a = ChangedRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        let b = ChangedRegion {
            x: 100,
            y: 100,
            width: 5,
            height: 5,
        };
        let bounds = bounding_region(&[a, b]);
        assert_eq!(bounds.bounds(), (10, 10, 95, 95));
        assert!(bounds.contains(&a) && bounds.contains(&b));

        let window = ChangedRegion {
            x: 25,
            y: 25,
            width: 10,
            height: 10,
        };
        assert!(overlaps(&a, &window));
        assert!(!overlaps(&b, &window));
        assert_eq!(intersect(&a, &window).bounds(), (25, 25, 5, 5));
    }

    // ── Fixtures ──────────────────────────────────────────────────────────

    fn test_state(width: u16, height: u16) -> ClientState {
        ClientState {
            view: Framebuffer::new(width, height),
            format: Fmt::BGRX32,
            preferences: EncodingPreferences::new(&[]),
            needs_full_refresh: Arc::new(AtomicBool::new(true)),
            pending: None,
        }
    }

    fn request_all(state: &mut ClientState, width: u16, height: u16) {
        state.pending = Some(PendingRequest {
            x: 0,
            y: 0,
            width,
            height,
        });
    }

    fn solid_frame(width: u16, height: u16) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        for pixel in fb.pixels_mut().chunks_exact_mut(4) {
            pixel.copy_from_slice(&[16, 32, 48, 255]);
        }
        fb
    }

    fn paint(fb: &mut Framebuffer, x: u16, y: u16, w: u16, h: u16, colour: [u8; 4]) {
        for row in y..y + h {
            for col in x..x + w {
                let at = row as usize * fb.stride() + col as usize * 4;
                fb.pixels_mut()[at..at + 4].copy_from_slice(&colour);
            }
        }
    }

    /// One solid colour per row, so a vertical scroll moves every pixel and the
    /// damage coalesces into a single full-frame rectangle.
    fn banded_frame(width: u16, height: u16) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        for y in 0..height {
            let colour = [(y as u8).wrapping_mul(3).wrapping_add(17), 200, 90, 255];
            for x in 0..width {
                paint(&mut fb, x, y, 1, 1, colour);
            }
        }
        fb
    }

    fn scrolled_frame(source: &Framebuffer, dx: i16, dy: i16) -> Framebuffer {
        let mut out = Framebuffer::new(source.width(), source.height());
        // after(x, y) == before(x + dx, y + dy), the convention `CopyRect`
        // encodes directly.
        let row_bytes = (source.width() as i32 - dx as i32).max(0) as usize * 4;
        let retained_height = source.height() as i32 - dy as i32;
        for y in 0..retained_height.max(0) as u16 {
            let start = y as usize * out.stride();
            let source_start = (y as i32 + dy as i32) as usize * source.stride()
                + dx as usize * 4;
            out.pixels_mut()[start..start + row_bytes]
                .copy_from_slice(&source.pixels()[source_start..source_start + row_bytes]);
        }
        let blank = Framebuffer::new(source.width(), source.height());
        for region in blank.exposed_by_scroll(dx, dy) {
            paint(&mut out, region.x, region.y, region.width, region.height, [7, 7, 7, 255]);
        }
        out
    }

    /// Total bytes a Tight rectangle occupies, given its payload from the control
/// byte onwards.
///
/// The compact length follows the control byte rather than preceding it, which
/// is the detail this walks twice by hand.
fn tight_payload_size(payload: &[u8]) -> usize {
    let first = payload[1];
    if first & 0x80 == 0 {
        // One byte: the length is itself.
        1 + 1 + first as usize
    } else {
        // Two or three bytes: the second byte's high bit says which, and the
        // seven-bit groups run low bits first.
        let low = (first as usize & 0x7F) | ((payload[2] as usize & 0x7F) << 7);
        if payload[2] & 0x80 == 0 {
            1 + 2 + low
        } else {
            1 + 3 + low + ((payload[3] as usize) << 14)
        }
    }
}
}
#[cfg(test)]
mod chord_tests {
    use super::*;

    /// X11 sends Ctrl+C as keysym 0x03, not as "c with Ctrl held". Without
    /// this mapping Ctrl+C, Ctrl+V and Ctrl+D are undeliverable, and the only
    /// symptom is a log line saying the key was unsupported.
    #[test]
    fn ctrl_control_characters_map_to_chords() {
        assert_eq!(keysym_to_sendkey(0x03).unwrap(), "ctrl-c");
        assert_eq!(keysym_to_sendkey(0x16).unwrap(), "ctrl-v");
        assert_eq!(keysym_to_sendkey(0x04).unwrap(), "ctrl-d");
        assert_eq!(keysym_to_sendkey(0x01).unwrap(), "ctrl-a");
    }

    #[test]
    fn alt_is_offset_from_latin1() {
        // Alt+a arrives as 0x0161.
        assert_eq!(keysym_to_sendkey(0x0161).unwrap(), "alt-a");
        // Alt+space arrives as 0x0120; HMP spells the space bar "spc".
        assert_eq!(keysym_to_sendkey(0x0120).unwrap(), "alt-spc");
    }

    /// A plain printable key must stay plain: the chord paths must not leak
    /// modifiers into ordinary typing.
    #[test]
    fn plain_printables_are_unmodified() {
        assert_eq!(keysym_to_sendkey(b'a' as u32).unwrap(), "a");
        assert_eq!(keysym_to_sendkey(b'A' as u32).unwrap(), "shift-a");
        assert_eq!(keysym_to_sendkey(b'1' as u32).unwrap(), "1");
        assert_eq!(keysym_to_sendkey(b'!' as u32).unwrap(), "shift-1");
    }

    /// Still refuses what it cannot map, rather than sending a plausible guess.
    #[test]
    fn unknown_keysyms_are_still_refused() {
        assert!(keysym_to_sendkey(0x0100_0001).is_err());
    }
}