//! `vnc-client`: the Aurora VNC viewer core.
//!
//! The crate is layered so each part can be replaced without touching the others:
//!
//! - [`framebuffer`]: an owned 0x00RRGGBB pixel buffer plus conversion from any
//!   RFB pixel format (true colour or colour map, 8/16/32 bpp, either endianness).
//! - [`session`]: a sans-I/O client session. Bytes in, [`Event`]s out; typed
//!   input calls in, bytes out. It drives `rfb-proto`'s `ClientHandshake` and
//!   then the normal message phase, so the host and client share one parser.
//! - [`net`]: a small blocking TCP driver over [`Session`] for tools and tests.
//!   A GUI or async runtime can drive [`Session`] directly instead.

#![forbid(unsafe_code)]

pub mod classic;
pub mod clipboard;
pub mod config;
pub mod cursor;
pub mod desktop;
pub mod framebuffer;
pub mod net;
pub mod session;
pub mod tight;
pub mod zrle;

pub use framebuffer::{Framebuffer, Rect};
pub use session::{compress_level, jpeg_quality, ClientError, Event, Session, SessionOptions, TlsOptions};

/// Re-exported so front ends can build TLS settings without a direct dependency.
pub use rfb_proto;
pub use rfb_tls;
