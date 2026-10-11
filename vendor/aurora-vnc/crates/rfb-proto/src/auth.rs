//! Classic VNC Authentication (RFC 6143 section 7.2.2).
//!
//! This is DES with a password of at most 8 bytes. It does not protect against
//! a network attacker on its own, so Aurora only offers it inside an encrypted
//! transport (VeNCrypt/TLS or WebSocket over TLS), and only when the operator
//! enables it for compatibility with older viewers.

use des::cipher::{BlockEncrypt, KeyInit};
use des::Des;

pub const CHALLENGE_LEN: usize = 16;

/// The 16-byte response to a VNC Authentication challenge.
///
/// The key is the password truncated or zero-padded to 8 bytes, with the bit
/// order of every byte reversed (a historical quirk of the original VNC code).
/// Each 8-byte half of the challenge is encrypted separately (DES-ECB).
pub fn vnc_auth_response(password: &[u8], challenge: &[u8; CHALLENGE_LEN]) -> [u8; CHALLENGE_LEN] {
    let mut key = [0u8; 8];
    for (k, p) in key.iter_mut().zip(password.iter()) {
        *k = p.reverse_bits();
    }
    let cipher = Des::new_from_slice(&key).expect("DES key is 8 bytes");
    let mut out = *challenge;
    for block in out.chunks_exact_mut(8) {
        cipher.encrypt_block(block.into());
    }
    key.fill(0);
    out
}

/// Compare two responses without leaking where they differ through timing.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
