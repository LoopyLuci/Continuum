# Continuum Security

## Current Security Posture

### What's Implemented

1. **Transport Encryption**: QUIC mandates TLS 1.3 — all data in transit is encrypted
2. **TOFU Certificate Pinning**: Client saves server cert fingerprint on first connect, warns on change
3. **Certificate Persistence**: Server TLS cert is generated once and persisted to disk
4. **Pairing Authentication**: Shared secret (`pairing_code`) required before access is granted
5. **Access Gating**: Media streams, input injection, clipboard, and file transfer require successful pairing
6. **Rate Limiting**: 5 pairing attempts per 60-second window per connection
7. **Session Tokens**: Generated on successful pairing for session management

### What's NOT Implemented

1. **Client Identity**: No persistent client identity beyond the session token
2. **Authorization Tokens**: No token-based auth for reconnection without re-pairing
3. **End-to-End Encryption**: `continuum-security` (double-ratchet) exists but is not wired into the transport
4. **Mutual TLS**: Server does not verify client certificates
5. **Audit Logging**: No persistent audit trail of connections and actions

## Security Architecture

### Connection Flow

```
Client                          Server
  │                               │
  ├─ TLS 1.3 Handshake ──────────┤
  │   (server presents cert)      │
  │                               │
  ├─ TOFU Pin Check               │
  │   (first use: trust & save)   │
  │   (subsequent: verify match)  │
  │                               │
  ├─ Open Intent Stream ──────────┤
  ├─ PairingHandshake ────────────┤
  │   {pairing_code, client_name} │
  │                               ├─ Rate limit check
  │                               ├─ Code comparison
  │   PairingResponse ←───────────┤
  │   {accepted, permissions}     │
  │                               │
  ├─ Open Media Stream ───────────┤
  │                               ├─ Verify paired status
  │   (frames start flowing)      │
  │                               │
  ├─ Input Events ────────────────┤
  │                               ├─ Verify paired + can_control
  │   (injected on host)          │
```

### Threat Model

| Threat | Mitigation | Status |
|--------|------------|--------|
| Eavesdropping | TLS 1.3 | Implemented |
| MITM | TOFU pinning | Implemented |
| Brute-force pairing | Rate limiting | Implemented |
| Unauthorized access | Pairing gate | Implemented |
| Replay attacks | QUIC nonce | Implemented |
| Server impersonation | TOFU warning | Implemented |
| DoS | Connection limits | Implemented |
| Session hijacking | Session tokens | Partial |

### Configuration

```toml
# Server security settings
pairing_code = "change-this-in-production"
persist_cert = true
rate_limit_attempts = 5
rate_limit_window_secs = 60
max_clients = 16
```

### Best Practices

1. **Change the default pairing code** — `"continuum"` is the default and should never be used in production
2. **Use strong pairing codes** — at least 12 characters, mixed case, numbers, symbols
3. **Limit max_clients** — reduce attack surface by limiting concurrent connections
4. **Monitor logs** — watch for repeated pairing failures (potential brute-force)
5. **Network segmentation** — don't expose the QUIC port to the internet without a relay
6. **Use the relay server** — for internet access, use the relay rather than direct exposure

## Future Security Enhancements

### Phase 1: TOTP Pairing
Replace static pairing code with time-based one-time passwords (like Google Authenticator).

### Phase 2: DID Handshake
Wire in `continuum-security`'s DID handshake for cryptographic identity verification.

### Phase 3: Double-Ratchet Encryption
Layer end-to-end encryption on top of QUIC transport using the double-ratchet protocol.

### Phase 4: Mutual TLS
Require client certificates for authentication, eliminating the need for pairing codes.

### Phase 5: Zero-Trust Architecture
Implement capability-based access control with fine-grained permissions per action.
