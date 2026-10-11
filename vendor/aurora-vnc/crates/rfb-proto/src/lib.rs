//! `rfb-proto`: a pure, sans-I/O implementation of the RFB protocol (RFC 6143).
//!
//! This crate never touches sockets, threads, clocks, or the OS. Callers feed it
//! bytes and get typed values back, and hand it typed values to get bytes out.
//! That keeps it portable, deterministic, and fuzzable, and lets the host and the
//! client share exactly one parser.
//!
//! Decoding convention: every `decode` takes a byte slice and returns either the
//! value plus how many bytes it consumed, `DecodeError::Incomplete` when more
//! bytes are needed (nothing is consumed), or `DecodeError::Invalid`.

#![forbid(unsafe_code)]

pub mod auth;
pub mod clipboard;
pub mod codec;
pub mod compat;
pub mod encodings;
pub mod handshake;
pub mod messages;
pub mod pseudo;
pub mod types;
pub mod vencrypt;
pub mod video;
pub mod websocket;

pub use clipboard::ExtClipboard;
pub use codec::{DecodeError, Limits};
pub use handshake::{ClientConfig, ClientHandshake, HandshakeError, ServerConfig, ServerHandshake, VncAuthConfig};
pub use messages::{ClientMessage, RectHeader, ServerMessage};
pub use pseudo::{validate_layout, AlphaCursor, Cursor, ExtendedDesktopSize, Screen};
pub use types::{ClientInit, Encoding, PixelFormat, SecurityType, ServerInit, Version};
pub use vencrypt::{InnerAuth, PlainCredentials, VeNCryptServerConfig, VeNCryptSubtype};
pub use video::{
    AuroraClientMessage, AuroraServerMessage, Codec, CodecCap, DecodePerf, TierPerf, VideoCaps, VideoConfig,
    VideoRectHeader,
};
