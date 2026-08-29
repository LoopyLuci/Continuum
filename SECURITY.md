# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Continuum, please report it responsibly:

1. **Do NOT open a public GitHub issue.**
2. Email security concerns to: [INSERT EMAIL]
3. Include a detailed description of the vulnerability.
4. We will acknowledge receipt within 48 hours.
5. We will provide a fix timeline within 7 days.

## Security Model

### Encryption
- All media frames are encrypted with AES-256-GCM using a double-ratchet protocol.
- Key exchange uses X25519 Diffie-Hellman authenticated via PAKE (password-authenticated key exchange).
- The pairing password is never transmitted over the network.
- Short Authentication Strings (SAS) are displayed for user verification.

### Authentication
- Connections require a 6-character pairing code.
- After first pairing, sessions can be resumed with a cryptographically random token.
- TLS 1.3 (via QUIC) provides transport-layer encryption.
- Certificate pinning with TOFU (Trust On First Use) model.

### Forward Secrecy
- DH ratchet steps occur every 200 messages, providing forward secrecy.
- Old chain keys are discarded after ratchet steps.
- Key material is zeroed on drop using the `zeroize` crate.

### Known Limitations
- The `--insecure` flag disables E2E encryption (intended for local testing only).
- Self-signed certificates are used by default; no CA infrastructure.
- PAKE provides offline-dictionary resistance but uses a 6-character password (limited entropy).

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | :white_check_mark: |
| < 0.2   | :x:                |

## Cryptographic Model (Plain Language)

### How Continuum Protects Your Data

When you connect to a remote computer with Continuum, your screen, keyboard input, clipboard, files, and audio are all encrypted. Here's how it works in simple terms:

**The Handshake (When You Connect)**
1. You enter a 6-character pairing code on both computers
2. Both computers use this code to perform a "password-authenticated key exchange" (PAKE)
3. This means the pairing code is NEVER sent over the network — both computers prove they know it without revealing it
4. Both computers display two words (the "Short Authentication String") — if they match, you know you're connected to the right computer and no one is intercepting

**The Encryption (During Your Session)**
1. All data is encrypted with AES-256-GCM — the same encryption used by banks and governments
2. Every 200 frames, the computers perform a new key exchange (DH ratchet) — this means if an attacker somehow steals one key, they can't decrypt past or future frames
3. When your session ends, all encryption keys are erased from memory

**What an attacker CAN'T do:**
- Read your screen, keystrokes, clipboard, or files (encrypted)
- Impersonate you or the server (PAKE authentication)
- Decrypt past sessions (forward secrecy via DH ratchet)
- Decrypt future sessions (new keys each time)

**What an attacker CAN do (known limitations):**
- If they know your 6-character pairing code AND are positioned as a man-in-the-middle during the initial connection, they could attempt to intercept (the SAS words would NOT match in this case — always verify!)
- A 6-character code has limited entropy (~28 bits) — this protects against network attackers but not someone who can guess the code
- The `--insecure` flag disables all encryption — only use for local testing
