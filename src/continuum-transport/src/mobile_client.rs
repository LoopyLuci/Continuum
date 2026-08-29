//! # Continuum — Mobile & Web Client Architecture
//!
//! This module documents the architecture for native mobile and web clients.

/// ## iOS Client Architecture
///
/// ### Transport
/// - `Network.framework` QUIC (built-in since iOS 15)
/// - ALPN: `"apq-2"` matching desktop protocol
/// - TLS 1.3 with certificate pinning (stored in Secure Enclave)
///
/// ### Media
/// - `VideoToolbox` H.265 hardware decoder
/// - `Metal` rendering pipeline via `MTKView`
/// - Target: <16ms decode-to-display at 1080p
///
/// ### Input
/// - Touch → mouse events with gesture mapping:
///   - Tap = left click
///   - Long press = right click
///   - Two-finger drag = scroll
///   - Pinch = zoom (client-side only)
///
/// ### Identity
/// - Secure Enclave stores persistent client key
/// - DID handshake on first connection
/// - Biometric unlock (FaceID/TouchID) for pairing code
///
/// ### Dependencies (Swift Package Manager)
/// ```swift
/// // No external QUIC stack needed — Network.framework provides it
/// import Network
/// import VideoToolbox
/// import Metal
/// import MetalKit
/// ```
///
/// ## Android Client Architecture
///
/// ### Transport
/// - `Cronet` QUIC stack (Google's implementation)
/// - Same ALPN and TLS configuration as desktop
///
/// ### Media
/// - `MediaCodec` H.265 decoder (API 21+)
/// - `OpenGL ES 3.1` or `Vulkan` rendering via `TextureView`
/// - Surface multiplexing for zero-copy decode-to-render
///
/// ### Input
/// - Touch event → mouse coordinate mapping
/// - Physical keyboard attachment support (Bluetooth keyboards)
/// - Stylus support with pressure sensitivity
///
/// ### Identity
/// - Android `KeyStore` for persistent identity
/// - Biometric prompt for pairing
///
/// ### Dependencies
/// ```kotlin
/// // build.gradle.kts
/// dependencies {
///     implementation("org.chromium.net:cronet-api:119.0")
///     implementation("org.chromium.net:cronet-embedded:119.0")
/// }
/// ```
///
/// ## Web Client Architecture
///
/// ### Transport
/// - WebRTC data channel (QUIC not available in all browsers)
/// - `str0m` WASM-compiled ICE/DTLS stack as alternative
/// - Fallback: WebSocket over HTTP/3 for restrictive networks
///
/// ### Media
/// - `WebCodecs` API for hardware-accelerated H.265/H.264 decode
/// - WebGL2 / WebGPU for compositing
/// - Canvas2D fallback for older browsers
///
/// ### Input
/// - Standard DOM mouse/keyboard/touch events
/// - Pointer Lock API for raw mouse movement (FPS-style)
/// - Gamepad API for controller support
///
/// ### Build
/// ```bash
/// wasm-pack build --target web --features wasm
/// npm run build  # or yarn build
/// ```
///
/// ### Service Worker
/// - Offline support via cache-first strategy
/// - Background sync for queued input events
/// - Install prompt for PWA
pub struct MobileClient;

impl MobileClient {
    pub fn available_platforms() -> &'static [&'static str] {
        &["iOS 15+", "Android 12+", "Web (Chrome/Edge/Safari 16+)"]
    }
}
