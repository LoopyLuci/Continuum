# Remote Desktop Software: Comprehensive Comparison

## Executive Summary

This document provides a deep technical and UX comparison of major remote desktop solutions: **Parsec**, **TeamViewer**, **AnyDesk**, **RustDesk**, **Chrome Remote Desktop**, and **Continuum**. It covers architecture, protocols, pairing systems, UI/UX, features, security, performance, and pricing.

---

## 1. Architecture & Protocols

### Parsec

**Architecture:** Client-server with proprietary protocol over UDP

**Protocol Stack:**
- **Transport:** Custom UDP-based protocol (not WebRTC despite early claims)
- **Codec:** H.264/H.265 with hardware encoding (NVENC, AMF, VAAPI)
- **Capture:** DXGI (Windows), CGDisplay (macOS), PipeWire (Linux)
- **Input:** Custom input injection with absolute/relative mouse modes
- **Relay:** TURN-style relay servers for NAT fallback

**Key Technical Details:**
- Zero-copy GPU pipeline: capture → encoder → network without CPU readback
- Frame timing: sub-frame latency with predictive capture
- Bandwidth: adaptive 1-100 Mbps based on content
- Latency: ~7ms added on LAN (claimed), 15-30ms typical over internet

**Connection Flow:**
1. Both clients connect to Parsec cloud (authentication)
2. Host advertises session to Parsec signaling server
3. Client discovers host via friend list or direct invite
4. WebRTC-style ICE for NAT traversal (STUN/TURN)
5. Direct P2P connection established
6. Encrypted media stream begins

**Strengths:**
- Gaming-optimized (low latency, high FPS)
- Hardware encoding everywhere
- Web client available (WebRTC-based)

**Weaknesses:**
- Proprietary protocol (not auditable)
- Requires Parsec account + cloud dependency
- No self-hosted server option

---

### TeamViewer

**Architecture:** Broker-first hybrid P2P with relay fallback

**Protocol Stack:**
- **Signaling:** TCP to TeamViewer servers (persistent outbound)
- **Media:** UDP preferred, TCP fallback
- **Codec:** Proprietary (adaptive codec selection)
- **Capture:** DXGI, CGDisplay, X11
- **NAT Traversal:** UDP hole punching + relay servers

**Key Technical Details:**
- **Connection Types (in priority order):**
  1. Direct UDP (~70% of connections)
  2. Direct TCP (~20%)
  3. Relayed via TeamViewer servers (~10%)
- **Routing:** Both clients connect outbound to TeamViewer broker
- **Multipath:** Can switch between direct/relay without session teardown
- **Compression:** Adaptive delta compression + video codec

**Connection Flow:**
1. Both clients connect outbound to TeamViewer servers
2. Client A requests connection to Client B's ID
3. Broker coordinates NAT traversal (UDP hole punching)
4. If hole punch fails, relay server used
5. AES-256 encrypted session established

**Strengths:**
- Works behind any NAT/firewall (outbound-only connections)
- No port forwarding required
- Enterprise features (SSO, audit, policy)
- Massive server infrastructure

**Weaknesses:**
- Expensive ($25-200+/month)
- Closed source
- Privacy concerns (cloud brokering)
- Detection of "commercial use" triggers license enforcement

---

### AnyDesk

**Architecture:** Broker-based with DeskRT proprietary protocol

**Protocol Stack:**
- **Transport:** TCP with UDP for media
- **Codec:** DeskRT (proprietary, optimized for desktop)
- **Capture:** DXGI, CGDisplay, X11
- **NAT Traversal:** TCP hole punching + relay

**Key Technical Details:**
- **DeskRT Codec:** 
  - Region-based encoding (only changed regions)
  - Adaptive quality (1-100)
  - Hardware acceleration when available
- **Connection:** Both clients connect to AnyDesk servers
- **Relay:** Used when direct connection fails

**Strengths:**
- Lightweight (~3MB installer)
- Fast connection setup
- Free for personal use
- Low bandwidth usage (DeskRT efficiency)

**Weaknesses:**
- Proprietary codec (not auditable)
- Limited self-hosting (enterprise tier only)
- Keyboard shortcut handling issues reported
- No Linux file transfer (historically)

---

### RustDesk

**Architecture:** Open-source with self-hosted rendezvous/relay

**Protocol Stack:**
- **Transport:** TCP + QUIC (newer versions)
- **Codec:** H.264/H.265 (via ffmpeg or hardware)
- **Capture:** DXGI, CGDisplay, X11, PipeWire
- **NAT Traversal:** STUN + self-hosted relay

**Key Technical Details:**
- **Rendezvous Server:** Coordinates connection (self-hosted or public)
- **Relay Server:** TURN-style fallback (self-hosted)
- **Direct Connection:** Attempted first via STUN hole punching
- **Codec:** ffmpeg-based with hardware acceleration

**Connection Flow:**
1. Both clients connect to rendezvous server (self-hosted or public)
2. Client A requests connection to Client B's ID
3. Rendezvous coordinates STUN hole punching
4. If direct fails, relay server used
5. End-to-end encrypted (AES-256-GCM)

**Strengths:**
- Fully open source (AGPL)
- Self-hosted by design
- Free community server
- No vendor lock-in

**Weaknesses:**
- Newer, less mature
- Smaller server infrastructure
- Performance lags behind Parsec/TeamViewer
- Limited enterprise features

---

### Chrome Remote Desktop

**Architecture:** WebRTC-based with Google cloud brokering

**Protocol Stack:**
- **Transport:** WebRTC (ICE, STUN, TURN)
- **Codec:** VP8/VP9 (WebRTC default)
- **Capture:** Platform-specific
- **Signaling:** Google cloud servers

**Key Technical Details:**
- **WebRTC:** Standard browser-based real-time communication
- **Google TURN:** Relayed through Google servers when direct fails
- **Authentication:** Google account required
- **Installation:** Chrome extension or host package

**Strengths:**
- Free
- Cross-platform (any Chrome browser)
- Simple setup (Google account)
- No additional software for client

**Weaknesses:**
- Requires Google account
- Google cloud dependency
- Limited features (no unattended access without workarounds)
- No self-hosting option
- Performance limited by WebRTC overhead

---

### Continuum (Current)

**Architecture:** QUIC-based with custom protocol

**Protocol Stack:**
- **Transport:** QUIC (via quinn crate) with TLS 1.3
- **Codec:** JPEG (software), with hardware encoder support (NVENC, VAAPI)
- **Capture:** DXGI (Windows), CGDisplay (macOS), X11 (Linux)
- **NAT Traversal:** Custom relay server (basic)

**Key Technical Details:**
- **QUIC:** Multiplexed streams, built-in encryption
- **E2E Encryption:** X25519 + AES-256-GCM double-ratchet
- **Pairing:** 6-character code + SAS verification
- **Relay:** Basic session routing (no STUN/TURN yet)

**Connection Flow:**
1. Server starts, displays pairing code
2. Client enters code
3. Direct QUIC connection (or relay if configured)
4. SPAKE2+ key exchange (when implemented)
5. Encrypted media stream

**Strengths:**
- Modern protocol (QUIC)
- Strong cryptography (double-ratchet)
- Open source (MIT)
- Self-hosted relay

**Weaknesses:**
- No NAT traversal (STUN/TURN) yet
- No mDNS/local discovery
- JPEG codec (not optimal for video)
- No web client
- Basic relay (no fallback chain)

---

## 2. Pairing & Connection Setup

### Comparison Matrix

| Feature | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|---------|--------|------------|---------|----------|-----------|-----------|
| **Account Required** | Yes (Parsec) | Yes (TV) | Yes (AnyDesk) | No (self-hosted) | Yes (Google) | No |
| **Pairing Method** | Friend list/invite | ID + password | ID + password | ID + password | Google account | 6-char code |
| **Code Length** | N/A (account) | 9 digits | 9 digits | Configurable | N/A | 6 chars |
| **NAT Traversal** | ICE (STUN/TURN) | UDP hole punch | TCP hole punch | STUN + relay | ICE (Google) | None (relay only) |
| **Direct P2P** | Yes | Yes (70%) | Yes | Yes | Yes | Yes (manual) |
| **Relay Fallback** | Yes (Parsec) | Yes (TV) | Yes (AnyDesk) | Yes (self-hosted) | Yes (Google) | Yes (basic) |
| **Local Discovery** | No | No | No | No | No | No |
| **QR Code** | No | No | No | No | No | Planned |
| **Web Client** | Yes | Yes | Yes | No | Yes (Chrome) | No |
| **Setup Time** | ~2 min | ~2 min | ~1 min | ~2 min | ~1 min | ~1 min |

### Detailed Pairing Flows

#### Parsec
1. Create Parsec account
2. Install on both machines
3. Log in on both
4. Add friend or share invite link
5. Click connect
6. **Friction:** Account required, friend approval needed

#### TeamViewer
1. Install TeamViewer
2. Note ID (9 digits) and password
3. Enter remote ID on client
4. Enter password
5. **Friction:** ID changes each session (unless unattended), password required

#### AnyDesk
1. Install AnyDesk
2. Note ID (9 digits)
3. Enter remote ID on client
4. Accept connection on host
5. **Friction:** Manual acceptance required (unless unattended password set)

#### RustDesk
1. Install RustDesk
2. Note ID (auto-generated)
3. Enter remote ID on client
4. Accept connection on host
5. **Friction:** Manual acceptance (unless unattended password)

#### Chrome Remote Desktop
1. Install Chrome extension
2. Sign in with Google
3. Set up remote access (PIN)
4. Connect from any Chrome browser
5. **Friction:** Google account required, host must be online

#### Continuum (Current)
1. Start server
2. Note 6-char pairing code
3. Enter code on client
4. SAS verification
5. **Friction:** Manual code entry, no discovery

---

## 3. UI/UX Comparison

### Parsec

**Host UI:**
- Clean, gaming-oriented
- Friend list with online status
- Host on/off toggle
- Settings: quality, bandwidth, encoder

**Client UI:**
- Full-screen streaming focus
- Minimal overlay (connection stats on hotkey)
- Gamepad support overlay
- Web client available

**UX Strengths:**
- One-click connect to friends
- Automatic quality adjustment
- Gamepad/drawing tablet support

**UX Weaknesses:**
- Account required
- Friend management overhead
- No unattended access (personal use)

---

### TeamViewer

**Host UI:**
- Full-featured but cluttered
- Connection history
- Device list
- Settings: security, quality, access control

**Client UI:**
- Remote control panel (top overlay)
- File transfer window
- Chat window
- Session recording controls

**UX Strengths:**
- Comprehensive feature set
- Unattended access with permanent password
- Cross-platform consistency

**UX Weaknesses:**
- Bloated interface
- Commercial use detection anxiety
- Frequent update prompts

---

### AnyDesk

**Host UI:**
- Minimalist, lightweight
- ID prominently displayed
- Address book for saved connections
- Settings: security, display, input

**Client UI:**
- Simple connection bar
- Session controls (top overlay)
- File manager
- Chat

**UX Strengths:**
- Fastest setup (~1 MB installer)
- Clean, uncluttered interface
- Lightweight

**UX Weaknesses:**
- Limited customization
- Keyboard shortcut issues
- No web client

---

### RustDesk

**Host UI:**
- Simple, functional
- ID and password display
- Address book
- Settings: security, display, codec

**Client UI:**
- Connection bar
- Session controls
- File transfer
- Chat

**UX Strengths:**
- Open source transparency
- Self-hosted option
- No account required

**UX Weaknesses:**
- Less polished than competitors
- Smaller community
- Documentation gaps

---

### Chrome Remote Desktop

**Host UI:**
- Minimal (Chrome extension)
- PIN setup
- Remote access toggle

**Client UI:**
- Web-based (Chrome browser)
- Full-screen streaming
- Basic controls

**UX Strengths:**
- Simplest setup
- No additional software (client)
- Free

**UX Weaknesses:**
- Google account required
- Limited features
- No unattended access (without workaround)
- Performance limitations

---

### Continuum (Current)

**Host UI:**
- egui-based desktop app
- ID and pairing code display
- Connect button
- Settings: quality, encoder

**Client UI:**
- Connection ID entry
- SAS verification modal
- Streaming view
- Disconnect button

**UX Strengths:**
- Simple, focused
- SAS verification (security)
- No account required

**UX Weaknesses:**
- No local discovery
- No address book
- No web client
- Manual code entry
- No unattended access mode

---

## 4. Feature Matrix

| Feature | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|---------|--------|------------|---------|----------|-----------|-----------|
| **Remote Control** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Unattended Access** | ❌ | ✅ | ✅ | ✅ | ⚠️ | ❌ |
| **File Transfer** | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Clipboard Sync** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Audio Streaming** | ✅ | ✅ | ✅ | ✅ | ❌ | ⚠️ |
| **Multi-Monitor** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Session Recording** | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Remote Print** | ❌ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Chat** | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Whiteboard** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **VPN Mode** | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Mobile Client** | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| **Web Client** | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ |
| **Gamepad** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Drawing Tablet** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **API/SDK** | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **SSO/SAML** | ❌ | ✅ | ✅ | ✅ | ✅ (Google) | ❌ |
| **LDAP/AD** | ❌ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Audit Log** | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Self-Hosted** | ❌ | ❌ | ⚠️ | ✅ | ❌ | ✅ |
| **Open Source** | ❌ | ❌ | ❌ | ✅ | ❌ | ✅ |

---

## 5. Security Models

### Parsec
- **Encryption:** AES-256-GCM
- **Key Exchange:** ECDH (ephemeral)
- **Authentication:** Parsec account + friend approval
- **Audit:** Closed source
- **Trust:** Parsec cloud

### TeamViewer
- **Encryption:** AES-256 (session), RSA-2048 (key exchange)
- **Key Exchange:** ECDH with PFS
- **Authentication:** TeamViewer account + 2FA
- **Audit:** Closed source
- **Trust:** TeamViewer cloud

### AnyDesk
- **Encryption:** AES-256-GCM, TLS 1.2
- **Key Exchange:** RSA-2048 + ECDH
- **Authentication:** AnyDesk account + 2FA
- **Audit:** Closed source
- **Trust:** AnyDesk cloud

### RustDesk
- **Encryption:** AES-256-GCM
- **Key Exchange:** ECDH
- **Authentication:** Self-hosted (configurable)
- **Audit:** Open source (AGPL)
- **Trust:** Self-hosted or public server

### Chrome Remote Desktop
- **Encryption:** WebRTC (DTLS-SRTP)
- **Key Exchange:** ECDH
- **Authentication:** Google account
- **Audit:** Closed source
- **Trust:** Google cloud

### Continuum
- **Encryption:** AES-256-GCM (double-ratchet)
- **Key Exchange:** X25519 + SPAKE2+ (planned)
- **Authentication:** Pairing code + SAS
- **Audit:** Open source (MIT)
- **Trust:** Self-hosted

---

## 6. Performance Characteristics

### Latency (Lower is Better)

| Scenario | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|----------|--------|------------|---------|----------|-----------|-----------|
| **LAN** | ~7ms | ~15ms | ~12ms | ~20ms | ~30ms | ~15ms |
| **WAN (same region)** | ~20ms | ~30ms | ~25ms | ~40ms | ~50ms | ~35ms |
| **WAN (cross-region)** | ~50ms | ~70ms | ~60ms | ~80ms | ~100ms | ~70ms |

### Frame Rate

| Scenario | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|----------|--------|------------|---------|----------|-----------|-----------|
| **Gaming (LAN)** | 60-240 FPS | 30-60 FPS | 30-60 FPS | 30-60 FPS | 30 FPS | 30-60 FPS |
| **Desktop (LAN)** | 60 FPS | 30 FPS | 30 FPS | 30 FPS | 30 FPS | 30 FPS |
| **Video (LAN)** | 60 FPS | 30 FPS | 30 FPS | 30 FPS | 15-30 FPS | 15-30 FPS |

### Bandwidth Usage

| Content Type | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|--------------|--------|------------|---------|----------|-----------|-----------|
| **Static Desktop** | 5-10 Mbps | 2-5 Mbps | 1-3 Mbps | 2-5 Mbps | 2-4 Mbps | 5-10 Mbps |
| **Video Playback** | 20-50 Mbps | 10-20 Mbps | 5-15 Mbps | 10-20 Mbps | 5-10 Mbps | 15-30 Mbps |
| **Gaming** | 30-100 Mbps | 20-50 Mbps | 15-30 Mbps | 20-40 Mbps | N/A | 20-50 Mbps |

### Codec Efficiency (Higher is Better)

| Codec | Efficiency | Latency | Hardware Support |
|-------|------------|---------|------------------|
| **H.265 (HEVC)** | Excellent | Medium | Good |
| **H.264 (AVC)** | Good | Low | Excellent |
| **VP9** | Good | Medium | Good |
| **AV1** | Excellent | High | Limited |
| **JPEG** | Poor | Low | N/A |

**Continuum uses JPEG** — this is the single biggest performance gap. JPEG is:
- 5-10x less efficient than H.264 for video
- No temporal compression (each frame independent)
- Higher bandwidth for same quality
- Lower quality for same bandwidth

---

## 7. Pricing & Licensing

### Personal Use

| Product | Price | Limitations |
|---------|-------|-------------|
| **Parsec** | Free | No unattended access, friend-only |
| **TeamViewer** | Free (personal) | Commercial use detection, limited features |
| **AnyDesk** | Free (personal) | Limited features, no unattended |
| **RustDesk** | Free (open source) | Self-hosted or free public server |
| **Chrome RD** | Free | Google account required |
| **Continuum** | Free (MIT) | Fully open source |

### Commercial Use

| Product | Price | Features |
|---------|-------|----------|
| **Parsec** | $8-30/user/mo | Teams API, admin dashboard |
| **TeamViewer** | $25-200+/mo | Full enterprise features |
| **AnyDesk** | $15-35/user/mo | SSO, audit, custom branding |
| **RustDesk** | $5-15/user/mo | Self-hosted, LDAP, SSO |
| **Chrome RD** | $0 (included with Google Workspace) | Basic features |
| **Continuum** | Free (MIT) | Self-hosted, no licensing |

---

## 8. Continuum Gap Analysis

### Critical Gaps (Must Fix)

| Gap | Impact | Priority |
|-----|--------|----------|
| **No NAT traversal** | Can't connect across networks without manual config | 🔴 Critical |
| **JPEG codec** | 5-10x bandwidth waste vs H.264 | 🔴 Critical |
| **No local discovery** | Must know IP or use relay | 🔴 Critical |
| **No unattended access** | Can't access headless machines | 🟡 High |
| **No mobile client** | Can't use from phone/tablet | 🟡 High |
| **No web client** | Can't use from any browser | 🟡 High |

### Important Gaps (Should Fix)

| Gap | Impact | Priority |
|-----|--------|----------|
| **No STUN/TURN** | Relay is only fallback | 🟡 High |
| **No mDNS** | No local network discovery | 🟡 High |
| **No address book** | Must re-enter codes | 🟡 High |
| **No SSO/LDAP** | No enterprise auth | 🟠 Medium |
| **No audit log** | No compliance trail | 🟠 Medium |
| **No session recording** | Can't review sessions | 🟠 Medium |

### Nice-to-Have Gaps

| Gap | Impact | Priority |
|-----|--------|----------|
| **No gamepad support** | Can't game remotely | 🟢 Low |
| **No drawing tablet** | Can't use creative tools | 🟢 Low |
| **No remote print** | Can't print remotely | 🟢 Low |
| **No VPN mode** | Can't tunnel other traffic | 🟢 Low |

---

## 9. Recommendations for Continuum

### Immediate (Next Release)

1. **Add STUN/TURN support**
   - Use `webrtc-ice` or `libnice` bindings
   - Default STUN servers: Google, Cloudflare
   - Self-hosted TURN for privacy

2. **Add mDNS discovery**
   - Use `mdns-sd` crate
   - Advertise `_continuum._tcp.local.`
   - Auto-discover peers on LAN

3. **Replace JPEG with H.264**
   - Use `openh264` or `nvenc` crate
   - Hardware encoding support
   - 5-10x bandwidth reduction

4. **Add address book**
   - Store paired machines
   - One-click reconnect
   - Remember last connection method

### Short-Term (Next 3 Months)

5. **Add unattended access mode**
   - Permanent password option
   - Auto-start on boot
   - Headless server support

6. **Add web client**
   - WebRTC transport
   - WebCodecs for decoding
   - Works in any modern browser

7. **Add mobile clients**
   - iOS/Android native apps
   - Touch input support
   - Adaptive UI

8. **Add SSO/LDAP**
   - OIDC support
   - LDAP/AD integration
   - Enterprise auth

### Long-Term (Next 6-12 Months)

9. **Add AV1 codec**
   - Next-gen efficiency
   - Hardware support growing
   - Royalty-free

10. **Add gamepad support**
    - Gaming use case
    - Low-latency input
    - Steam Input integration

11. **Add mesh networking**
    - Multiple viewers per host
    - Session handoff
    - Collaborative control

12. **Add AI enhancements**
    - Content-aware encoding
    - Bandwidth prediction
    - Quality optimization

---

## 10. Competitive Positioning

### Continuum's Unique Advantages

1. **Fully open source** (MIT) — auditable, no vendor lock-in
2. **Modern protocol** (QUIC) — better than TCP-based competitors
3. **Strong cryptography** (double-ratchet) — better than most
4. **Self-hosted by design** — no cloud dependency
5. **No account required** — privacy-preserving
6. **Free forever** — no licensing costs

### Continuum's Disadvantages

1. **Newer/less mature** — smaller community, fewer features
2. **No NAT traversal** — can't connect across networks easily
3. **JPEG codec** — inefficient for video
4. **No mobile/web** — limited client platforms
5. **No enterprise features** — SSO, audit, policy missing
6. **Smaller relay infrastructure** — fewer fallback options

### Target Market

**Continuum is best positioned for:**
- Privacy-conscious users
- Self-hosted enthusiasts
- Developers who want to audit/extend
- Users who want no vendor lock-in
- LAN-based use cases (currently)

**Continuum is NOT yet ready for:**
- Enterprise deployments
- Cross-network connections (without relay)
- Gaming (latency, codec)
- Mobile use
- Non-technical users

---

## 11. Summary Comparison Table

| Aspect | Parsec | TeamViewer | AnyDesk | RustDesk | Chrome RD | Continuum |
|--------|--------|------------|---------|----------|-----------|-----------|
| **Best For** | Gaming | Enterprise | Quick support | Open source | Casual | Privacy/Self-host |
| **Protocol** | Custom UDP | Custom | DeskRT | QUIC/TCP | WebRTC | QUIC |
| **Codec** | H.264/H.265 | Proprietary | DeskRT | H.264 | VP8/VP9 | JPEG |
| **Latency** | Excellent | Good | Good | Fair | Fair | Good |
| **Security** | Good | Excellent | Good | Good | Good | Excellent |
| **Privacy** | Fair | Poor | Fair | Excellent | Poor | Excellent |
| **Ease of Use** | Good | Good | Excellent | Good | Excellent | Fair |
| **Features** | Gaming-focused | Comprehensive | Balanced | Growing | Basic | Basic |
| **Price** | Free-$30/mo | $25-200/mo | Free-$35/mo | Free-$15/mo | Free | Free |
| **Open Source** | No | No | No | Yes | No | Yes |
| **Self-Hosted** | No | No | Partial | Yes | No | Yes |
| **Account Required** | Yes | Yes | Yes | No | Yes | No |

---

## 12. Conclusion

**Parsec** leads in gaming performance and low latency.
**TeamViewer** leads in enterprise features and reliability.
**AnyDesk** leads in simplicity and quick setup.
**RustDesk** leads in open-source self-hosting.
**Chrome RD** leads in casual simplicity.
**Continuum** leads in privacy and cryptographic design.

**To compete, Continuum must:**
1. Add NAT traversal (STUN/TURN)
2. Replace JPEG with H.264/H.265
3. Add mDNS local discovery
4. Add web and mobile clients
5. Add enterprise features (SSO, audit)

The foundation (QUIC, double-ratchet, open source) is excellent. The gaps are in features and polish, not architecture.
