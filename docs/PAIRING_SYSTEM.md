# Zero-Friction Pairing & Connection System

## Executive Summary

This document specifies a complete zero-friction pairing and connection system for Continuum. The goal: two machines connect with **zero networking knowledge, zero IP addresses, zero configuration** — just a short pairing code or QR scan.

**Design principles:**
1. **Zero knowledge** — users never need to know IPs, ports, or network topology
2. **Zero configuration** — automatic discovery, automatic NAT traversal, automatic encryption
3. **Minimal data** — pairing codes are short (6-8 characters), all key exchange is derived
4. **Multiple paths** — local discovery, relay, direct — all automatic
5. **Cryptographic identity** — machines have persistent identities, pairing is binding identity to identity
6. **100-year durability** — modular, versioned, algorithm-agile

---

## 1. System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           CONNECTION LIFECYCLE                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐              │
│  │ DISCOVER │───▶│  PAIR    │───▶│ CONNECT  │───▶│  STREAM  │              │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘              │
│       │               │               │               │                     │
│       ▼               ▼               ▼               ▼                     │
│  mDNS/QR/Relay    SPAKE2+        ICE+QUIC       Encrypted                  │
│  find peer        derive key     traverse NAT    media flow                 │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                           DISCOVERY METHODS                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Priority 1: mDNS (same LAN)                                               │
│    └─ Automatic, zero-config, finds peers on local network                 │
│                                                                             │
│  Priority 2: QR Code (proximity)                                           │
│    └─ Scan QR on remote machine, contains pairing token + relay info       │
│                                                                             │
│  Priority 3: Relay Server (any network)                                    │
│    └─ Both machines connect to known relay, relay routes by pairing code   │
│                                                                             │
│  Priority 4: Manual (fallback)                                             │
│    └─ Enter pairing code, system tries all discovery methods automatically │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Identity System

### 2.1 Machine Identity

Every Continuum instance has a persistent cryptographic identity:

```rust
pub struct MachineIdentity {
    /// Ed25519 signing key (persistent, generated on first run)
    pub signing_key: Ed25519SecretKey,
    /// X25519 key agreement key (derived from signing key or independent)
    pub agreement_key: X25519SecretKey,
    /// Human-readable name (user-configurable)
    pub display_name: String,
    /// Unique machine ID (hash of public key, for dedup)
    pub machine_id: [u8; 32],
}
```

**Storage:** `~/.config/continuum/identity.json` (encrypted at rest with OS keychain when available)

**Properties:**
- Generated once on first run, never changes unless explicitly rotated
- Machine ID is `Blake3(signing_public_key)[0..8]` → 16 hex chars for display
- Identity is independent of network location (IP can change, identity persists)

### 2.2 Identity Verification

When pairing, users verify identity via **SAS (Short Authentication String)**:

```
Machine A: "Alice's Laptop" [a3f7b2d9]
Machine B: "Bob's Desktop"  [e8c1a4f2]

SAS Words: "correct horse battery staple"
```

SAS is derived from the SPAKE2 shared secret, guaranteeing no MITM.

---

## 3. Pairing Code System

### 3.1 Pairing Code Format

```
Format: XXXX-XXXX (8 chars, alphanumeric, case-insensitive)
Example: "a3f7-e8c1"

Encoding: 40 bits of entropy (2^40 = ~1 trillion codes)
Alphabet: 0123456789abcdefghjkmnpqrstvwxyz (no ambiguous chars: i, l, o, u)
Checksum: Last char is Luhn-like checksum for typo detection
```

**Why 8 chars?**
- Short enough to type quickly
- High enough entropy for brute-force resistance (with rate limiting)
- Fits in QR codes easily
- Works on any input method (keyboard, voice, etc.)

### 3.2 Pairing Code Types

| Type | Format | Use Case | Lifetime |
|------|--------|----------|----------|
| **One-time** | `a3f7-e8c1` | Single use, expires after pairing or 10 min | 10 min |
| **Persistent** | `alice-home` | User-defined name, never expires | Forever |
| **Session** | `sess-abc123` | Temporary session for guest access | Configurable |
| **QR** | (encoded URL) | Scan to pair, contains all info | 10 min |

### 3.3 Pairing Code Generation

```rust
pub fn generate_pairing_code() -> String {
    // 40 bits of CSPRNG entropy
    let mut rng = rand::thread_rng();
    let entropy: [u8; 5] = rng.gen();
    
    // Encode to 8 chars from alphabet
    let alphabet = "0123456789abcdefghjkmnpqrstvwxyz";
    let mut code = String::with_capacity(9); // 8 chars + dash
    
    // Convert 40 bits to 8 base-32 chars
    let mut bits = u64::from_be_bytes([0, 0, 0, 0, entropy[0], entropy[1], entropy[2], entropy[3]]);
    for i in 0..8 {
        let idx = (bits >> (35 - i * 5)) & 0x1F;
        code.push(alphabet.chars().nth(idx as usize).unwrap());
        if i == 3 { code.push('-'); }
    }
    
    code
}
```

### 3.4 Pairing Code Security

- **Rate limiting:** Max 10 attempts per minute per IP/machine
- **Expiry:** One-time codes expire after 10 minutes
- **Single use:** Code is consumed on successful pairing
- **No offline brute-force:** SPAKE2 requires online interaction per attempt
- **Checksum:** Typo detection prevents wasted attempts

---

## 4. Discovery Layer

### 4.1 mDNS/Bonjour (Local Network)

**Protocol:** mDNS (RFC 6762) + DNS-SD (RFC 6763)

```rust
pub struct MdnsDiscovery {
    /// Service type: "_continuum._tcp.local."
    service_type: String,
    /// Our machine identity
    identity: MachineIdentity,
    /// Current pairing code (if advertising)
    pairing_code: Option<String>,
}
```

**How it works:**
1. Server starts, advertises `_continuum._tcp.local.` with TXT records:
   - `machine_id=a3f7b2d9...`
   - `display_name=Alice's Laptop`
   - `pairing_code=a3f7-e8c1` (if active)
   - `protocol_version=1`
2. Client browses for `_continuum._tcp.local.` services
3. Client resolves hostname + port from SRV record
4. Direct connection attempted (no relay needed)

**Implementation:** Use `mdns-sd` crate (pure Rust, cross-platform)

**Fallback:** If mDNS is blocked (some corporate networks), fall through to relay.

### 4.2 QR Code Discovery

**QR contains a URI:**
```
continuum://pair?code=a3f7-e8c1&name=Alice%27s%20Laptop&relay=relay.example.com
```

**QR generation:**
```rust
pub fn generate_pairing_qr(pairing_code: &str, display_name: &str, relay: &str) -> QrCode {
    let uri = format!(
        "continuum://pair?code={}&name={}&relay={}",
        pairing_code,
        url_encode(display_name),
        relay
    );
    QrCode::with_error_correction_level(&uri, EcLevel::M) // 15% error correction
}
```

**QR scanning:**
- Client has built-in QR scanner (camera or screen capture)
- Parse URI, extract pairing code
- Attempt connection via all available methods

### 4.3 Relay-Assisted Discovery

**When mDNS fails (different networks, NAT, firewalls):**

1. Both machines connect to a known relay server (configurable, default: `relay.continuum.local`)
2. Server advertises: "I want to pair with code `a3f7-e8c1`"
3. Relay matches pairing codes, routes traffic between matched machines
4. After pairing, machines attempt direct connection (ICE hole-punch)
5. If direct fails, continue relaying

**Relay protocol:**
```rust
pub struct RelaySession {
    pub session_id: String,
    pub pairing_code: String,
    pub host_machine_id: [u8; 32],
    pub peer_machine_id: Option<[u8; 32]>,
    pub state: RelayState,
}

pub enum RelayState {
    WaitingForPeer,
    PeerConnected,
    DirectConnectionEstablished,
    Closed,
}
```

### 4.4 Discovery Priority

```
1. Check local cache (previously paired machines)
2. mDNS browse (same LAN)
3. QR scan (proximity)
4. Relay (any network)
5. Manual code entry (fallback)
```

All methods are tried automatically. User only needs to enter a code if all automatic methods fail.

---

## 5. Pairing Protocol

### 5.1 SPAKE2+ (Password-Authenticated Key Exchange)

**Why SPAKE2+?**
- Both parties share a weak password (pairing code)
- Derives a strong shared key without transmitting the password
- Resistant to offline dictionary attacks
- Mutual authentication (both parties prove knowledge of password)
- Used by Apple HomeKit, Magic Wormhole, Noise PSK mode

**Protocol flow:**

```
Machine A (Initiator)                          Machine B (Responder)
─────────────────────                          ─────────────────────
1. Generate random scalar a                    1. Generate random scalar b
2. Compute A = g^a * M^code                    2. Compute B = g^b * N^code
3. Send A ─────────────────────────────────▶  3. Receive A
                                               4. Compute K = (A * M^code)^(-b) = g^ab
4. Receive B ◀──────────────────────────────── 5. Send B
5. Compute K = (B * N^code)^(-a) = g^ab
6. Derive session key from K                   6. Derive session key from K
```

**Implementation:** Use `spake2` crate (pure Rust, audited)

### 5.2 Noise XX Handshake (with PSK)

After SPAKE2 establishes a shared secret, use Noise XX for the transport handshake:

```
XX Pattern:
  -> e
  <- e, ee, s, se
  -> s, se
```

**With PSK mode (from SPAKE2):**
```
Noise_XX+PSK:
  -> psk, e
  <- e, ee, s, se
  -> s, se
```

**Benefits:**
- Mutual authentication (both sides prove identity)
- Forward secrecy (ephemeral keys per session)
- Zero-RTT encryption (with PSK, first message is encrypted)
- Identity hiding (public keys encrypted)

### 5.3 Complete Pairing Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        COMPLETE PAIRING FLOW                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Machine A                              Machine B                           │
│  ─────────                              ─────────                           │
│                                                                             │
│  1. Generate pairing code "a3f7-e8c1"   1. Display pairing code            │
│  2. Advertise via mDNS + relay          2. Scan QR / browse mDNS           │
│  3. Wait for connection attempt         3. Enter code / select from list    │
│                                         4. Connect to A (via mDNS/relay)   │
│                                                                             │
│  4. Receive connection ◀──────────────── 5. Send SPAKE2 initiator message   │
│  5. Send SPAKE2 responder message ─────▶ 6. Compute shared secret           │
│  6. Compute shared secret               7. Send Noise XX + PSK handshake    │
│  7. Complete Noise handshake ◀───────── 8. Verify SAS words match          │
│  8. Verify SAS words match                                                 │
│                                                                             │
│  9. Display SAS: "correct horse"        9. Display SAS: "correct horse"    │
│  10. User confirms match                10. User confirms match            │
│                                                                             │
│  11. Pairing complete!                  11. Pairing complete!              │
│  12. Store paired identity              12. Store paired identity          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 5.4 SAS (Short Authentication String)

**Derivation:**
```rust
fn derive_sas(shared_secret: &[u8]) -> String {
    // HKDF derive 32-bit value from shared secret
    let mut hkdf = Hkdf::<Sha256>::new(None, shared_secret);
    let mut okm = [0u8; 4];
    hkdf.expand(b"continuum-sas", &mut okm).unwrap();
    
    // Map to word list (BIP39-style, 2048 words)
    let wordlist = load_wordlist();
    let idx = u32::from_be_bytes(okm) % 2048;
    
    // Use 2 words for ~11 bits of entropy (good enough for short codes)
    let idx1 = (idx >> 5) & 0x7FF;
    let idx2 = idx & 0x7FF;
    
    format!("{} {}", wordlist[idx1], wordlist[idx2])
}
```

**Why words instead of numbers?**
- Easier to read aloud (phone support)
- Easier to compare (less error-prone)
- Works across languages (localized word lists)

---

## 6. Connection Layer

### 6.1 ICE (Interactive Connectivity Establishment)

**Goal:** Find the best path between two machines, traversing NATs and firewalls.

**ICE candidates (in priority order):**

| Priority | Type | Description | When Used |
|----------|------|-------------|-----------|
| 1 | Host | Local IP:port | Same machine (loopback) |
| 2 | Server Reflexive | Public IP:port via STUN | Different machines, same NAT |
| 3 | Peer Reflexive | Discovered during check | NAT hairpinning |
| 4 | Relayed | TURN server | Symmetric NAT, firewall |

**STUN servers (configurable, defaults):**
- `stun:stun1.l.google.com:19302`
- `stun:stun2.l.google.com:19302`
- `stun:stun.continuum.local` (self-hosted)

**TURN servers (fallback):**
- `turn:turn.continuum.local:3478` (self-hosted coturn)

### 6.2 QUIC Transport

**After ICE establishes connectivity:**

1. QUIC connection to the best ICE candidate
2. Noise XX+PSK handshake inside QUIC (mutual auth)
3. Bidirectional streams for media, input, clipboard, files

**QUIC configuration:**
```rust
pub struct QuicConfig {
    /// ALPN protocols
    pub alpn: Vec<Vec<u8>>, // ["continuum-1"]
    /// Idle timeout
    pub idle_timeout: Duration, // 30s
    /// Keep-alive interval
    pub keep_alive_interval: Duration, // 10s
    /// Max concurrent streams
    pub max_concurrent_streams: u64, // 100
    /// Stream receive window
    pub stream_receive_window: u64, // 8 MB
    /// Connection receive window
    pub connection_receive_window: u64, // 16 MB
}
```

### 6.3 Connection State Machine

```rust
pub enum ConnectionState {
    /// Initial state
    Idle,
    /// Discovering peer (mDNS, relay, QR)
    Discovering,
    /// Pairing in progress (SPAKE2, Noise)
    Pairing {
        saspake_state: Spake2State,
        noise_state: NoiseState,
    },
    /// Verifying SAS
    VerifyingSas {
        sas: String,
    },
    /// ICE connectivity checking
    ConnectingIce {
        candidates: Vec<IceCandidate>,
    },
    /// QUIC handshake
    ConnectingQuic,
    /// Connected, encrypted
    Connected {
        connection: QuicConnection,
    },
    /// Connection lost, attempting reconnect
    Reconnecting {
        attempt: u32,
        next_attempt: Instant,
    },
    /// Disconnected
    Disconnected {
        reason: DisconnectReason,
    },
}
```

### 6.4 Reconnection Strategy

```rust
pub struct ReconnectPolicy {
    /// Initial backoff
    initial_delay: Duration, // 1s
    /// Maximum backoff
    max_delay: Duration, // 30s
    /// Backoff multiplier
    multiplier: f32, // 2.0
    /// Maximum attempts (0 = infinite)
    max_attempts: u32, // 0
    /// Use exponential backoff with jitter
    jitter: bool, // true
}

impl ReconnectPolicy {
    pub fn next_delay(&self, attempt: u32) -> Duration {
        let delay = self.initial_delay * self.multiplier.powi(attempt as i32);
        let delay = delay.min(self.max_delay);
        
        if self.jitter {
            // Add ±25% jitter to avoid thundering herd
            let jitter = rand::random::<f32>() * 0.5 - 0.25;
            Duration::from_secs_f32(delay.as_secs_f32() * (1.0 + jitter))
        } else {
            delay
        }
    }
}
```

---

## 7. Trust Model

### 7.1 Trust on First Use (TOFU)

```rust
pub struct TrustStore {
    /// Map of machine_id -> trusted public key
    trusted_machines: HashMap<[u8; 32], TrustedMachine>,
}

pub struct TrustedMachine {
    pub display_name: String,
    pub public_key: Ed25519PublicKey,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub pairing_method: PairingMethod,
}

pub enum PairingMethod {
    /// Paired via one-time code + SAS verification
    OneTimeCode { sas: String },
    /// Paired via QR code scan
    QrCode,
    /// Paired via mDNS (local network trust)
    Mdns,
    /// Manually trusted (imported key)
    Manual,
}
```

### 7.2 Key Pinning

After first pairing, the public key is pinned. If it changes:

1. **Warning:** "This machine's identity has changed. Re-pair to confirm."
2. **Block:** Connection blocked until user explicitly re-pairs
3. **Audit:** Log the key change for security review

### 7.3 Revocation

```rust
pub struct RevocationList {
    /// List of revoked machine IDs
    revoked: HashSet<[u8; 32]>,
    /// Last updated
    last_updated: DateTime<Utc>,
    /// Source of revocation list
    source: RevocationSource,
}

pub enum RevocationSource {
    /// Local user revocation
    Local,
    /// From a central server (enterprise)
    Server(String),
    /// Distributed (gossip protocol)
    Distributed,
}
```

---

## 8. Network Architecture

### 8.1 Connection Methods (Priority Order)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                      CONNECTION METHOD PRIORITY                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  1. LOCAL CACHE                                                             │
│     └─ Previously paired? Try last known address first                     │
│                                                                             │
│  2. mDNS (same LAN)                                                        │
│     └─ Browse _continuum._tcp.local.                                       │
│     └─ Resolve SRV record → hostname + port                                │
│     └─ Direct connection (no relay needed)                                 │
│                                                                             │
│  3. ICE + STUN (different networks)                                        │
│     └─ Gather candidates (host, srflx, relay)                              │
│     └─ Connectivity checks                                                 │
│     └─ Best candidate wins                                                 │
│                                                                             │
│  4. RELAY (when direct fails)                                              │
│     └─ Connect to relay.continuum.local                                    │
│     └─ Relay routes by pairing code                                       │
│     └─ Attempts ICE hole-punch in background                              │
│                                                                             │
│  5. MANUAL (fallback)                                                      │
│     └─ User enters code, system tries all methods                          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 8.2 Relay Server Architecture

```rust
pub struct RelayServer {
    /// Active sessions indexed by pairing code
    sessions: HashMap<String, RelaySession>,
    /// Active sessions indexed by machine ID
    machine_sessions: HashMap<[u8; 32], Vec<String>>,
}

pub struct RelaySession {
    pub pairing_code: String,
    pub host: Option<RelayPeer>,
    pub peer: Option<RelayPeer>,
    pub created_at: Instant,
    pub state: RelayState,
}

pub struct RelayPeer {
    pub machine_id: [u8; 32],
    pub connection: QuicConnection,
    pub connected_at: Instant,
}

impl RelayServer {
    /// Handle a new relay connection
    pub async fn handle_connection(&mut self, connection: QuicConnection, pairing_code: String) {
        // Check if session exists
        if let Some(session) = self.sessions.get_mut(&pairing_code) {
            if session.host.is_none() {
                session.host = Some(RelayPeer::new(connection));
            } else if session.peer.is_none() {
                session.peer = Some(RelayPeer::new(connection));
                session.state = RelayState::PeerConnected;
                
                // Start forwarding between host and peer
                self.start_forwarding(pairing_code).await;
            }
        } else {
            // Create new session
            let session = RelaySession::new(pairing_code.clone(), connection);
            self.sessions.insert(pairing_code, session);
        }
    }
    
    /// Forward data between host and peer
    async fn start_forwarding(&self, pairing_code: String) {
        // Bidirectional copy between host and peer streams
        // With buffering and flow control
    }
}
```

### 8.3 NAT Traversal

**STUN (Session Traversal Utilities for NAT):**
- RFC 5389
- Discovers public IP:port
- Works for full-cone, restricted-cone, port-restricted NAT

**TURN (Traversal Using Relays around NAT):**
- RFC 5766
- Fallback for symmetric NAT
- Relays all traffic (higher latency, more bandwidth)

**ICE (Interactive Connectivity Establishment):**
- RFC 8445
- Coordinates STUN + TURN
- Finds best path automatically

**Implementation:** Use `webrtc-ice` crate or `libnice` bindings

---

## 9. User Experience

### 9.1 First Run Experience

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        FIRST RUN WIZARD                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Step 1: Identity                                                           │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  Welcome to Continuum!                                               │   │
│  │                                                                     │   │
│  │  This machine needs a name:                                         │   │
│  │  ┌─────────────────────────────────────────────────────────────┐   │   │
│  │  │ Alice's Laptop                                              │   │   │
│  │  └─────────────────────────────────────────────────────────────┘   │   │
│  │                                                                     │   │
│  │  Machine ID: a3f7b2d9e8c1... (auto-generated, unique)             │   │
│  │                                                                     │   │
│  │                    [ Continue ]                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  Step 2: Network                                                            │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  Network Setup                                                       │   │
│  │                                                                     │   │
│  │  ☑ Enable local network discovery (mDNS)                            │   │
│  │  ☑ Enable relay server (for connections outside your network)       │   │
│  │  ☐ Use custom relay server: [relay.example.com        ]             │   │
│  │                                                                     │   │
│  │  ☑ Allow direct connections (ICE/STUN)                              │   │
│  │  STUN servers:                                                      │   │
│  │  ☑ stun:stun1.l.google.com:19302                                   │   │
│  │  ☑ stun:stun2.l.google.com:19302                                   │   │
│  │  ☐ Custom STUN: [                              ]                   │   │
│  │                                                                     │   │
│  │                    [ Continue ]                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  Step 3: Security                                                           │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  Security Settings                                                   │   │
│  │                                                                     │   │
│  │  Pairing code length:                                               │   │
│  │  ○ 6 characters (faster, less secure)                               │   │
│  │  ● 8 characters (recommended)                                       │   │
│  │  ○ 12 characters (most secure)                                      │   │
│  │                                                                     │   │
│  │  Require SAS verification:                                          │   │
│  │  ● Always (recommended)                                             │   │
│  │  ○ Only for new machines                                            │   │
│  │  ○ Never (trusted networks only)                                    │   │
│  │                                                                     │   │
│  │  Auto-accept connections from:                                      │   │
│  │  ○ No one (always ask)                                              │   │
│  │  ● Previously paired machines                                       │   │
│  │  ○ Anyone on local network                                          │   │
│  │                                                                     │   │
│  │                    [ Finish ]                                       │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 9.2 Pairing Flow (User View)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        PAIRING FLOW                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Machine A (Host)                           Machine B (Client)              │
│  ───────────────                            ───────────────                  │
│                                                                             │
│  ┌─────────────────────────────────┐       ┌─────────────────────────────────┐
│  │  Home Screen                     │       │  Home Screen                     │
│  │                                 │       │                                 │
│  │  Your ID: a3f7b2d9              │       │  Connect to someone              │
│  │  Your Name: Alice's Laptop      │       │                                 │
│  │                                 │       │  ┌─────────────────────────┐   │
│  │  ┌─────────────────────────┐   │       │  │ Scan QR code             │   │
│  │  │ Generate Pairing Code    │   │       │  └─────────────────────────┘   │
│  │  └─────────────────────────┘   │       │                                 │
│  │                                 │       │  ┌─────────────────────────┐   │
│  │  Pairing Code: a3f7-e8c1       │       │  │ Enter pairing code       │   │
│  │  ┌─────┬─────┬─────┬─────┐   │       │  └─────────────────────────┘   │
│  │  │ a   │ 3   │ f   │ 7   │   │       │                                 │
│  │  ├─────┼─────┼─────┼─────┤   │       │  ┌─────────────────────────┐   │
│  │  │ -   │     │     │     │   │       │  │ Or browse local network: │   │
│  │  ├─────┼─────┼─────┼─────┤   │       │  │ • Alice's Laptop (a3f7) │   │
│  │  │ e   │ 8   │ c   │ 1   │   │       │  │ • Bob's Desktop (e8c1)  │   │
│  │  └─────┴─────┴─────┴─────┘   │       │  └─────────────────────────┘   │
│  │                                 │       │                                 │
│  │  [📷 Show QR Code]             │       │  Entered: a3f7-e8c1             │
│  │                                 │       │  [Connect]                      │
│  └─────────────────────────────────┘       └─────────────────────────────────┘
│                                                                             │
│  ┌─────────────────────────────────┐       ┌─────────────────────────────────┐
│  │  Pairing Request                 │       │  Verifying Identity              │
│  │                                 │       │                                 │
│  │  Bob's Desktop wants to connect │       │  Is this correct?                │
│  │                                 │       │                                 │
│  │  Machine ID: e8c1a4f2          │       │  Machine: Bob's Desktop          │
│  │  Name: Bob's Desktop            │       │  ID: e8c1a4f2                   │
│  │                                 │       │                                 │
│  │  Verify these words match:      │       │  Verify these words match:       │
│  │                                 │       │                                 │
│  │  ┌─────────────────────────┐   │       │  ┌─────────────────────────┐   │
│  │  │ correct horse           │   │       │  │ correct horse           │   │
│  │  └─────────────────────────┘   │       │  └─────────────────────────┘   │
│  │                                 │       │                                 │
│  │  [✓ Words Match] [✗ Reject]    │       │  [✓ Words Match] [✗ Reject]    │
│  └─────────────────────────────────┘       └─────────────────────────────────┘
│                                                                             │
│  ┌─────────────────────────────────┐       ┌─────────────────────────────────┐
│  │  Connected!                      │       │  Connected!                      │
│  │                                 │       │                                 │
│  │  Streaming Bob's Desktop        │       │  Streaming to Alice's Laptop    │
│  │                                 │       │                                 │
│  │  ┌─────────────────────────┐   │       │  ┌─────────────────────────┐   │
│  │  │                         │   │       │  │                         │   │
│  │  │   [Remote Desktop]      │   │       │  │   [Remote Desktop]      │   │
│  │  │                         │   │       │  │                         │   │
│  │  └─────────────────────────┘   │       │  └─────────────────────────┘   │
│  │                                 │       │                                 │
│  │  Quality: 85% | Latency: 12ms  │       │  Quality: 85% | Latency: 12ms  │
│  │  [Disconnect] [Settings]       │       │  [Disconnect] [Settings]       │
│  └─────────────────────────────────┘       └─────────────────────────────────┘
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 9.3 Connection Persistence

```rust
pub struct ConnectionManager {
    /// Active connections
    connections: HashMap<[u8; 32], Connection>,
    /// Known peers (from previous pairings)
    known_peers: HashMap<[u8; 32], KnownPeer>,
    /// Auto-reconnect settings
    auto_reconnect: bool,
}

pub struct KnownPeer {
    pub machine_id: [u8; 32],
    pub display_name: String,
    pub last_connected: DateTime<Utc>,
    pub last_known_addresses: Vec<String>,
    pub pairing_method: PairingMethod,
    pub trusted: bool,
}

impl ConnectionManager {
    /// Auto-reconnect to previously paired peer
    pub async fn auto_reconnect(&mut self, machine_id: &[u8; 32]) -> Result<(), Error> {
        let peer = self.known_peers.get(machine_id).ok_or(Error::UnknownPeer)?;
        
        // Try last known addresses first
        for addr in &peer.last_known_addresses {
            if let Ok(conn) = self.try_connect(addr).await {
                self.connections.insert(*machine_id, conn);
                return Ok(());
            }
        }
        
        // Fall back to discovery
        self.discover_and_connect(machine_id).await
    }
}
```

---

## 10. Implementation Roadmap

### Phase 1: Foundation (Weeks 1-2)
- [ ] Implement `MachineIdentity` with persistent storage
- [ ] Implement pairing code generation and validation
- [ ] Implement SPAKE2+ for key exchange
- [ ] Implement SAS derivation and display

### Phase 2: Discovery (Weeks 3-4)
- [ ] Implement mDNS discovery (mdns-sd crate)
- [ ] Implement QR code generation and scanning
- [ ] Implement relay server session management
- [ ] Implement discovery priority system

### Phase 3: Connection (Weeks 5-6)
- [ ] Implement ICE candidate gathering
- [ ] Implement STUN client
- [ ] Implement TURN client (fallback)
- [ ] Implement connection state machine

### Phase 4: Integration (Weeks 7-8)
- [ ] Integrate with existing QUIC transport
- [ ] Integrate with existing Noise handshake
- [ ] Implement reconnection with exponential backoff
- [ ] Implement trust store and key pinning

### Phase 5: UX (Weeks 9-10)
- [ ] Implement first-run wizard
- [ ] Implement pairing flow UI
- [ ] Implement connection status display
- [ ] Implement settings and configuration

### Phase 6: Hardening (Weeks 11-12)
- [ ] Security audit of pairing protocol
- [ ] Fuzz testing of protocol parsers
- [ ] Performance testing under NAT
- [ ] Documentation and examples

---

## 11. Security Considerations

### 11.1 Threat Model

| Threat | Mitigation |
|--------|------------|
| MITM during pairing | SPAKE2 + SAS verification |
| Replay attacks | Nonces in SPAKE2, ephemeral keys in Noise |
| Brute-force pairing codes | Rate limiting, short expiry, online-only |
| Relay server compromise | End-to-end encryption, relay can't read traffic |
| Stolen identity key | Key rotation, revocation lists |
| Quantum computing | Algorithm agility, post-quantum KEM ready |

### 11.2 Pairing Code Security

- **Entropy:** 40 bits (1 trillion possibilities)
- **Rate limit:** 10 attempts/minute per machine
- **Expiry:** 10 minutes for one-time codes
- **Single use:** Consumed on successful pairing
- **No offline attack:** SPAKE2 requires online interaction

### 11.3 Post-Quantum Readiness

```rust
pub enum KeyExchangeAlgorithm {
    X25519,
    Kyber512,  // NIST Round 3 finalist
    Kyber768,
    Kyber1024,
    X25519Kyber768,  // Hybrid (Cloudflare, Google)
}

pub struct CryptoProvider {
    algorithm: KeyExchangeAlgorithm,
    // ...
}

impl CryptoProvider {
    /// Negotiate algorithm with peer
    fn negotiate(&self, peer_algorithms: &[KeyExchangeAlgorithm]) -> KeyExchangeAlgorithm {
        // Prefer hybrid, then strongest shared
        // ...
    }
}
```

---

## 12. Configuration

### 12.1 Default Configuration

```toml
# ~/.config/continuum/config.toml

[identity]
# Auto-generated on first run, can be rotated
# machine_id = "a3f7b2d9..."
# display_name = "Alice's Laptop"

[network]
# mDNS discovery on local network
mdns_enabled = true

# Use relay server for NAT traversal
relay_enabled = true
relay_server = "relay.continuum.local"

# ICE/STUN for direct connections
ice_enabled = true
stun_servers = [
    "stun:stun1.l.google.com:19302",
    "stun:stun2.l.google.com:19302",
]

# TURN fallback
turn_enabled = true
turn_server = "turn:turn.continuum.local:3478"

[pairing]
# Pairing code length (6, 8, or 12)
code_length = 8

# Require SAS verification
require_sas = true

# Auto-accept from paired machines
auto_accept_paired = true

# Pairing code expiry (seconds)
code_expiry = 600

[security]
# Key exchange algorithm preference
key_exchange = "X25519Kyber768"

# Enable post-quantum algorithms
post_quantum = true

# Store paired machine keys
trust_store_path = "~/.config/continuum/trust.json"

[connection]
# Auto-reconnect settings
auto_reconnect = true
reconnect_initial_delay_ms = 1000
reconnect_max_delay_ms = 30000
reconnect_max_attempts = 0  # infinite

# Connection timeout
connect_timeout_ms = 10000

# Idle timeout (0 = never)
idle_timeout_ms = 0
```

### 12.2 Enterprise Configuration

```toml
# /etc/continuum/enterprise.toml

[identity]
# Use hardware security module for key storage
hsm_enabled = true
hsm_provider = "pkcs11"
hsm_library = "/usr/lib/softhsm/libsofthsm2.so"

[network]
# Use enterprise relay
relay_server = "relay.corporate.example.com"
relay_tls_cert = "/etc/continuum/relay-ca.pem"

# Custom STUN/TURN
stun_servers = ["stun:stun.corporate.example.com:3478"]
turn_server = "turn:turn.corporate.example.com:3478"
turn_username = "continuum"
turn_password = "secret"

[pairing]
# Require admin approval for new pairings
require_admin_approval = true
admin_email = "admin@example.com"

# Use directory service for identity verification
directory_enabled = true
directory_url = "ldap://ldap.corporate.example.com"
directory_base_dn = "ou=users,dc=example,dc=com"

[security]
# Enforce post-quantum algorithms
post_quantum = true
key_exchange = "X25519Kyber768"

# Certificate pinning
pin_relay_certificate = true
relay_certificate_pin = "sha256/abc123..."

[audit]
# Enable detailed audit logging
audit_enabled = true
audit_path = "/var/log/continuum/audit.jsonl"
audit_include_frames = true
```

---

## 13. API Reference

### 13.1 Core Types

```rust
/// Machine identity
pub struct MachineIdentity {
    pub signing_key: Ed25519SecretKey,
    pub agreement_key: X25519SecretKey,
    pub display_name: String,
    pub machine_id: [u8; 32],
}

/// Pairing code
pub struct PairingCode {
    pub code: String,
    pub created_at: Instant,
    pub expires_at: Instant,
    pub code_type: PairingCodeType,
}

pub enum PairingCodeType {
    OneTime,
    Persistent,
    Session,
}

/// Connection manager
pub struct ConnectionManager {
    connections: HashMap<[u8; 32], Connection>,
    known_peers: HashMap<[u8; 32], KnownPeer>,
}

/// Discovery service
pub struct DiscoveryService {
    mdns: Option<MdnsDiscovery>,
    relay: Option<RelayDiscovery>,
    qr: Option<QrDiscovery>,
}
```

### 13.2 Core Functions

```rust
/// Generate a new pairing code
pub fn generate_pairing_code(length: u8) -> PairingCode;

/// Validate a pairing code format
pub fn validate_pairing_code(code: &str) -> bool;

/// Derive SAS from shared secret
pub fn derive_sas(shared_secret: &[u8]) -> String;

/// Start advertising for pairing
pub async fn advertise_pairing(code: &str) -> Result<(), Error>;

/// Stop advertising
pub async fn stop_advertising() -> Result<(), Error>;

/// Browse for available peers
pub async fn browse_peers() -> Vec<PeerInfo>;

/// Connect to a peer by pairing code
pub async fn connect_with_code(code: &str) -> Result<Connection, Error>;

/// Connect to a known peer
pub async fn connect_to_peer(machine_id: &[u8; 32]) -> Result<Connection, Error>;

/// Disconnect from a peer
pub async fn disconnect(machine_id: &[u8; 32]) -> Result<(), Error>;

/// Get connection status
pub fn connection_status(machine_id: &[u8; 32]) -> ConnectionState;

/// Get list of known peers
pub fn known_peers() -> Vec<KnownPeer>;

/// Forget a paired machine
pub fn forget_peer(machine_id: &[u8; 32]) -> Result<(), Error>;
```

---

## 14. Testing Strategy

### 14.1 Unit Tests

```rust
#[test]
fn test_pairing_code_generation() {
    let code = generate_pairing_code(8);
    assert_eq!(code.code.len(), 9); // 8 chars + dash
    assert!(validate_pairing_code(&code.code));
}

#[test]
fn test_pairing_code_uniqueness() {
    let codes: HashSet<String> = (0..1000).map(|_| generate_pairing_code(8).code).collect();
    assert_eq!(codes.len(), 1000); // All unique
}

#[test]
fn test_sas_derivation() {
    let secret = b"test-secret";
    let sas1 = derive_sas(secret);
    let sas2 = derive_sas(secret);
    assert_eq!(sas1, sas2); // Deterministic
}

#[test]
fn test_spake2_roundtrip() {
    let password = b"a3f7-e8c1";
    let (msg_a, state_a) = spake2_initiate(password);
    let (msg_b, state_b) = spake2_respond(password, &msg_a);
    let key_a = spake2_finish(state_a, &msg_b).unwrap();
    let key_b = spake2_finish(state_b, &msg_a).unwrap();
    assert_eq!(key_a, key_b); // Same shared secret
}
```

### 14.2 Integration Tests

```rust
#[tokio::test]
async fn test_mdns_discovery() {
    let mut discovery = MdnsDiscovery::new().await.unwrap();
    discovery.advertise("test-peer", "a3f7-e8c1").await.unwrap();
    
    let peers = discovery.browse().await.unwrap();
    assert!(peers.iter().any(|p| p.name == "test-peer"));
}

#[tokio::test]
async fn test_relay_pairing() {
    let relay = RelayClient::connect("relay.continuum.local").await.unwrap();
    let session = relay.create_session("a3f7-e8c1").await.unwrap();
    
    let peer = relay.wait_for_peer(&session).await.unwrap();
    assert_eq!(peer.pairing_code, "a3f7-e8c1");
}

#[tokio::test]
async fn test_full_pairing_flow() {
    // Machine A generates code
    let code = generate_pairing_code(8);
    
    // Machine B connects with code
    let conn = connect_with_code(&code.code).await.unwrap();
    
    // Verify connection is encrypted
    assert!(conn.is_encrypted());
    
    // Verify SAS matches
    assert_eq!(conn.local_sas(), conn.remote_sas());
}
```

### 14.3 Fuzz Testing

```rust
#[fuzz]
fn fuzz_pairing_code_parser(data: &[u8]) {
    let _ = validate_pairing_code(&String::from_utf8_lossy(data));
}

#[fuzz]
fn fuzz_spake2_message(data: &[u8]) {
    let _ = Spake2Message::parse(data);
}

#[fuzz]
fn fuzz_ice_candidate(data: &[u8]) {
    let _ = IceCandidate::parse_sdp(&String::from_utf8_lossy(data));
}
```

---

## 15. Migration from Current System

### 15.1 Backward Compatibility

The new pairing system is designed to be backward-compatible with the existing system:

1. **Existing pairing codes** (6-char) continue to work
2. **Existing paired machines** are migrated to the new trust store
3. **Existing connections** use the same QUIC transport
4. **New features** (mDNS, QR, relay) are opt-in via configuration

### 15.2 Migration Steps

```rust
pub fn migrate_from_legacy() -> Result<(), Error> {
    // 1. Check for legacy identity
    if let Some(legacy) = load_legacy_identity()? {
        // 2. Convert to new format
        let identity = MachineIdentity::from_legacy(legacy);
        identity.save()?;
    }
    
    // 3. Check for legacy paired machines
    if let Some(legacy_peers) = load_legacy_peers()? {
        // 4. Convert to new trust store
        let trust_store = TrustStore::from_legacy(legacy_peers);
        trust_store.save()?;
    }
    
    // 5. Mark migration complete
    set_migration_flag()?;
    
    Ok(())
}
```

---

## 16. Summary

This system provides:

1. **Zero-friction pairing** — 6-8 character codes, QR scanning, mDNS auto-discovery
2. **Zero networking knowledge** — automatic NAT traversal, relay fallback
3. **Zero configuration** — works out of the box with sensible defaults
4. **Strong security** — SPAKE2+, Noise XX, SAS verification, key pinning
5. **100-year durability** — modular traits, algorithm agility, protocol versioning
6. **Enterprise ready** — SSO, audit logging, admin approval, HSM support

The architecture is designed so that every component can be replaced as technology evolves, without breaking the overall system.
