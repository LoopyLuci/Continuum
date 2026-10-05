//! RFB 3.8 server that streams one QEMU guest's framebuffer to VNC clients.
//!
//! ## Why this exists
//!
//! The real streaming path today is [`crate::ws_sidecar`], which polls QMP
//! `screendump` and ships a whole JPEG per poll. Every poll costs a full
//! framebuffer dump, a PNG decode, a resize, a JPEG encode and a full-frame
//! write, measured at 40-70 ms each — so throughput sits at 15-25 fps and a
//! static desktop costs exactly as much bandwidth as a scrolling one.
//!
//! RFB exists for that second half of the problem. The server keeps a
//! framebuffer, diffs it against the previous one, and sends *only the changed
//! rectangles*. A cursor blink or a character typed in a terminal is a few
//! hundred bytes. The guest's screen is transmitted once and then only as
//! deltas.
//!
//! ## What this does and does not fix — the honest version
//!
//! * **Fixed:** per-frame *bandwidth* and *client decode cost*. Unchanged
//!   regions cost zero bytes. This is the screendump problem gone, and it is
//!   why a mostly-static desktop streams at kilobytes per second instead of
//!   megabytes per second.
//! * **Fixed:** latency between a change happening in the guest and the change
//!   reaching the client is now bounded by one poll interval rather than by a
//!   PNG/JPEG encode of the whole screen.
//! * **NOT fixed:** the *capture poll rate*. [`crate::capture_qmp::QmpCaptureBackend`]
//!   still issues one `screendump` per poll because QMP has no "give me the
//!   changed region" command — `screendump` is the only framebuffer export path
//!   QEMU has, and it is file-based and full-frame. So this server is still
//!   bounded at roughly 15-25 fps by the capture, and claiming 60 fps would be
//!   false. What RFB buys is that the *upstream* limit is now the only limit:
//!   the wire no longer scales with how much of the screen is moving.
//! * **The obvious next step** is a dirty-region or DMA export from QEMU (the
//!   `DISPLAY_UPDATE`/`virgl` surfaces, or a change-detection shim in the
//!   guest). Until that exists, `screendump` remains the ceiling and this
//!   module documents it rather than pretending otherwise.
//!
//! ## Security posture
//!
//! This is a keyboard and mouse into a real guest, so:
//!
//! * **VNC Authentication (DES challenge/response) is required by default.**
//!   See [`auth`] for the RFC 6143 implementation and for what it does and
//!   does not protect.
//! * The listener binds **loopback by default** and the bind address is
//!   configurable but never defaulted to anything else.
//! * Passwords, challenge responses and derived key material are never logged
//!   and are zeroized on drop.
//! * Everything a client can influence is bounded: framebuffer dimensions,
//!   incoming message sizes, client count, per-client queue depth, and the
//!   idle timeout. See [`server::RfbServerConfig`].
//!
//! Disabling authentication requires explicitly setting
//! [`server::RfbServerConfig::allow_none_auth`]. It exists for a loopback-only
//! debugging session; there is no reason to set it on a server reachable from
//! another host, because an unauthenticated RFB connection is unrestricted
//! remote control of the guest with no credential and no audit trail.
//!
//! ## Encodings
//!
//! [`encoder`] implements Raw, CopyRect, RRE, Hextile and Tight, plus the
//! DesktopSize pseudo-encoding. ZRLE and the other pseudo-encodings are not
//! implemented; see the module for the exact list and the reasoning.
//!
//! ## Module layout
//!
//! * [`proto`] — constants, version/security negotiation, pixel formats,
//!   client message decoding.
//! * [`auth`] — VNC Authentication (DES challenge/response).
//! * [`encoder`] — framebuffer, damage tracking, per-encoding writers.
//! * [`server`] — the listener, per-client tasks, the update loop, input.
//!
//! The submodules are public so that the wire types can be named without going
//! through the re-exports below; the re-exports exist because almost every
//! caller wants `rfb::PixelFormat`, not `rfb::proto::PixelFormat`.

pub mod auth;
pub mod encoder;
pub mod proto;
pub mod server;

pub use auth::{generate_challenge, VncPassword, CHALLENGE_LEN, PASSWORD_LEN, RESPONSE_LEN};
pub use encoder::{
    all_contained, covers_most_of, full_frame, hextile_aligned, split_width, total_area,
    write_copy_rect, write_desktop_size, ChangedRegion, Encoding, EncodingPreferences, Framebuffer,
    Scroll, MAX_HEXTILE_TILE, MAX_RECTS_PER_UPDATE, MAX_TIGHT_RECT_WIDTH,
};
pub use proto::{
    decide_version, negotiate_security_type, parse_client_version, security_type_offer,
    ClientMessage, PixelFormat, Rect, SecurityType, ServerInit, VersionDecision,
    DEFAULT_DESKTOP_NAME, HIGHEST_SUPPORTED_VERSION, MAX_CLIENT_ENCODINGS, MAX_CUT_TEXT_BYTES,
    MAX_DESKTOP_NAME, SECURITY_INVALID, SECURITY_NONE, SECURITY_VNC_AUTH,
    SERVER_VERSION_BANNER, SUPPORTED_VERSIONS,
};
pub use server::{
    keysym_to_sendkey, RfbError, RfbServer, RfbServerConfig, DEFAULT_IDLE_TIMEOUT,
    DEFAULT_MAX_CLIENTS, DEFAULT_PORT, DEFAULT_WRITE_QUEUE_DEPTH, HANDSHAKE_TIMEOUT,
    MAX_VERSION_RETRIES,
};

/// Largest framebuffer the server will ever allocate, in pixels.
///
/// At the default 32 bpp that is 16 MiB per client, times
/// [`server::RfbServerConfig::max_clients`]. The cap exists because the
/// framebuffer is derived from guest-reported geometry: a client that asks for
/// a 30000x30000 desktop must not be able to make the server allocate 3.6 GB.
pub const MAX_FRAMEBUFFER_PIXELS: u32 = 4_194_304;

/// Largest framebuffer dimension the server will honour, in pixels.
pub const MAX_FRAMEBUFFER_DIMENSION: u32 = 8192;