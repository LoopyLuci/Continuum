pub mod audio;
pub mod benchmarks;
pub mod capture;
pub mod client;
pub mod codec;
pub mod config;
pub mod datagram;
pub mod debug_tunnel;
pub mod e2e;
pub mod error;
pub mod gpu_encoder;
pub mod gpu_pipeline;
pub mod ice_transport;
pub mod input;
pub mod lan_discovery;
pub mod mobile_client;
pub mod network_enterprise;
pub mod plugin;
pub mod qr_code;
pub mod recording;
pub mod server;
pub mod streaming_engine;
pub mod tls;
pub mod types;
#[cfg(feature = "wasm-plugins")]
pub mod wasm_plugin;
pub mod wgpu_compositor;

pub use capture::*;
pub use client::{
    connect_to_server, read_frame, send_clipboard_data, send_file, ConnectionEvent,
    ConnectionHealth, ConnectionMetrics, ControlCommand, ReconnectPolicy,
};
pub use codec::{decode_jpeg, encode_jpeg, AdaptiveEncoder};
pub use config::{ClientArgs, ClientConfig, ServerArgs, ServerConfig};
pub use lan_discovery::{DiscoveredServer, LanDiscovery};
pub use qr_code::PairingQr;
pub use server::run_server;
pub use types::*;
