//! VNC Authentication (RFC 6143 §7.2.2): a 16-byte DES challenge/response.
//!
//! ## The exchange
//!
//! ```text
//! S: 16 random bytes                        (the challenge)
//! C: DES(challenge, key)                    (the response)
//! S: SecurityResult
//! ```
//!
//! ## The three details that make it fiddly
//!
//! **1. The key is bit-reversed, per byte.** This is not in RFC 6143; it is
//! what every implementation actually does, and it is why so many ports get a
//! silently-always-wrong auth. ASCII's high bit is always zero, so reversing
//! each byte makes DES's internal key-bit drop land on a bit that carries no
//! information, instead of on a bit that is part of the password. The password
//! `"COW"` (`0x43 0x4F 0x57`) is used as the key `0xC2 0xF2 0xEA`.
//!
//! **2. The key is truncated or NUL-padded to exactly eight bytes.** A password
//! longer than eight characters *silently* uses only the first eight. This
//! server therefore refuses to start with a password longer than eight bytes
//! rather than accepting one whose tail is ignored.
//!
//! **3. DES is applied twice, as ECB over two independent blocks.** There is no
//! chaining and no IV. A 16-byte challenge is two 8-byte plaintexts, encrypted
//! separately.
//!
//! ## What this does and does not protect
//!
//! DES-56 is broken and an eight-character key is short. Against an attacker who
//! can observe a handshake and record responses this is offline-crackable in
//! minutes. It protects against exactly one thing: an unauthenticated local
//! process, or a network peer, being able to *use* the session — because
//! possessing the response requires possessing the password. That is the
//! threat model for a loopback-bound server, and it is why this is the default
//! rather than a non-option.
//!
//! It is not a substitute for transport encryption. RFC 6143 says so itself:
//! "not intended for use on untrusted networks", and recommends IPsec or SSH.
//! Binding away from loopback should come with a tunnel.
//!
//! Nothing in this module logs. [`VncPassword`] has no `Debug` impl that could
//! print the password, its `Display` prints a placeholder, and the buffer is
//! zeroized on drop. Passwords, responses and challenges all appear only as
//! `key_for` inputs to a `tracing` field name that is never given their value.
//!
//! # The cryptography is aurora's, the policy is this module's
//!
//! The DES-ECB of RFC 6143 §7.2.2 -- bit-reversed key, zero padding, two
//! independent blocks, constant-time comparison -- is
//! [`rfb_proto::auth`], and is used directly rather than reimplemented.
//!
//! That is not only de-duplication. A second implementation of an algorithm
//! this old is a second place for the historical quirks to go wrong, and this
//! project has already shipped three protocol bugs of exactly that shape. What
//! stays here is the part `rfb-proto` deliberately does not have:
//!
//! * the refusal to accept a password longer than eight bytes, which RFC 6143
//!   would silently truncate (see [`VncPassword::new`]);
//! * redaction and zeroization of the secret, so that no log line and no
//!   debugger can reveal it;
//! * the typed error the server turns into a refusal.
//!
//! `rfb_proto::auth::vnc_auth_response` takes the plaintext and derives the key
//! per call, so holding the password rather than the derived key is what
//! delegation costs. Zeroization below is what makes that acceptable.

use zeroize::Zeroize;

/// Bytes in the server's challenge.
pub const CHALLENGE_LEN: usize = 16;

/// Bytes in the client's response.
pub const RESPONSE_LEN: usize = 16;

/// Bytes in a DES key.
pub const PASSWORD_LEN: usize = 8;

/// An 8-byte VNC Authentication password.
///
/// Constructed from plaintext, which is not kept beyond the zeroized copy held
/// here. Continuum used to hold the *derived* DES key instead, which is one
/// step further from the secret; delegating to `rfb-proto` means the key is
/// derived per challenge, so the plaintext is what has to be protected.
pub struct VncPassword {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for VncPassword {
    /// Deliberately unhelpful. A `#[derive(Debug)]` here would put the password
    /// into any log line that so much as mentions a failed login.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VncPassword(<redacted>)")
    }
}

impl Drop for VncPassword {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl std::fmt::Display for VncPassword {
    /// Redacted. A `Display` impl is what makes `{}` in a log line safe; leaving
    /// it off would push the responsibility onto every call site.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VncPassword(<redacted>)")
    }
}

impl VncPassword {
    /// Validate a plaintext password.
    ///
    /// # Errors
    ///
    /// Rejects a password longer than [`PASSWORD_LEN`] characters. Accepting it
    /// would be a silent security downgrade: RFC 6143 truncates, so the ninth
    /// character would have no effect and an operator who believes it does would
    /// be wrong. `rfb_proto::auth::vnc_auth_response` truncates without comment,
    /// so this check is the only thing standing between an operator and a
    /// password they think is longer than it is. The upper bound is
    /// [`PASSWORD_LEN`] *bytes*, and the check is on bytes rather than `char`s
    /// because the key is eight bytes long.
    pub fn new(password: &str) -> Result<Self, PasswordError> {
        let bytes = password.as_bytes();
        if bytes.is_empty() {
            return Err(PasswordError::Empty);
        }
        if bytes.len() > PASSWORD_LEN {
            return Err(PasswordError::TooLong {
                len: bytes.len(),
                limit: PASSWORD_LEN,
            });
        }
        Ok(Self {
            bytes: bytes.to_vec(),
        })
    }

    /// The response to `challenge`.
    ///
    /// Delegates to `rfb_proto::auth`, which derives the bit-reversed key and
    /// encrypts each eight-byte half of the challenge separately (DES-ECB).
    pub fn response(&self, challenge: &[u8; CHALLENGE_LEN]) -> [u8; RESPONSE_LEN] {
        rfb_proto::auth::vnc_auth_response(&self.bytes, challenge)
    }

    /// Check a client's response against `challenge`, in constant time.
    ///
    /// Constant-time because the response is the only thing an attacker can
    /// measure: an early-exit comparison here would leak how many leading bytes
    /// of a guessed response were right. Eight bytes is far too little to brute
    /// force byte-by-byte anyway, but a comparison that leaks is a bug waiting
    /// for a better attacker, and the cost of accumulation is zero here.
    pub fn verify(&self, challenge: &[u8; CHALLENGE_LEN], response: &[u8]) -> bool {
        if response.len() != RESPONSE_LEN {
            return false;
        }
        rfb_proto::auth::constant_time_eq(&self.response(challenge), response)
    }

    /// Derive the response for a challenge without keeping a password object.
    ///
    /// Used by tests and by the integration-test client, which needs to answer
    /// a challenge rather than check one.
    pub fn response_for(password: &str, challenge: &[u8; CHALLENGE_LEN]) -> Result<[u8; RESPONSE_LEN], PasswordError> {
        Ok(Self::new(password)?.response(challenge))
    }
}

/// Why a password could not be turned into a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("the VNC password is empty; an empty key is not a credential")]
    Empty,

    #[error(
        "the VNC password is {len} bytes but DES keys are {limit}; RFC 6143 truncates \
         silently, so a longer password would appear to be stronger than it is"
    )]
    TooLong { len: usize, limit: usize },
}

/// Generate a fresh 16-byte challenge from the OS entropy source.
///
/// A predictable challenge is the whole attack: with it, an observer who does
/// not know the password can precompute responses for every candidate, and
/// replay them later against the same challenge. This is the CVE-2026-44040
/// failure mode, so it comes from `OsRng` and nowhere else — never from a
/// clock, a pid, or a counter.
pub fn generate_challenge() -> [u8; CHALLENGE_LEN] {
    use rand::RngCore;
    let mut challenge = [0u8; CHALLENGE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut challenge);
    challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn password_rejects_empty_and_overlong_input() {
        assert_eq!(VncPassword::new("").unwrap_err(), PasswordError::Empty);
        let err = VncPassword::new("123456789").unwrap_err();
        assert_eq!(
            err,
            PasswordError::TooLong {
                len: 9,
                limit: 8
            }
        );
        // Exactly eight is fine, and nine is not: the boundary is the point.
        assert!(VncPassword::new("12345678").is_ok());
    }

    #[test]
    fn the_challenge_response_vector_is_correct() {
        // A full end-to-end vector for the *VNC* layer: key formation (pad and
        // bit-reverse) plus two ECB blocks over a 16-byte challenge.
        //
        //   password "sesame"      73 65 73 61 6d 65 00 00
        //   key after reversal      ce a6 ce 86 b6 a6 00 00
        //   challenge              6b 5f 3e 21 08 6f 0c 35 7f 61 4a 1d 2c 5e 08 30
        //   response               f1 c6 4c 59 94 fc 13 94 f9 a9 c0 01 36 22 02 4d
        //
        // This is the test that matters after the move to `rfb_proto::auth`.
        // The DES primitive and the bit reversal are no longer ours, so the
        // per-key vector tests that pinned them went with them -- they tested
        // code that is not here any more. What must survive is that this
        // module still produces RFC 6143 bytes, and the key formation is only
        // observable through the response. The expected value was produced by
        // an independently written, table-driven DES operating on bit lists
        // rather than packed words, and checked against the published FIPS
        // 46-3 vector before being trusted.
        //
        // So a failure here means the delegation is wrong somewhere -- a
        // swapped convention, a wrong slice -- and not that the cipher is.
        let password = VncPassword::new("sesame").unwrap();
        let challenge: [u8; 16] = [
            0x6b, 0x5f, 0x3e, 0x21, 0x08, 0x6f, 0x0c, 0x35, 0x7f, 0x61, 0x4a, 0x1d, 0x2c, 0x5e,
            0x08, 0x30,
        ];
        let response = password.response(&challenge);
        assert_eq!(hex(&response), "f1c64c5994fc1394f9a9c0013622024d");
        assert!(password.verify(&challenge, &response));
    }

    #[test]
    fn the_two_blocks_are_encrypted_independently() {
        // ECB, not CBC: a challenge whose halves are equal must produce a
        // response whose halves are equal. A chaining mode would not.
        let password = VncPassword::new("sesame").unwrap();
        let mut challenge = [0x5au8; 16];
        challenge[..8].copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        let response = password.response(&challenge);
        assert_ne!(&response[..8], &response[8..]);

        let repeated = password.response(&[0x42; 16]);
        assert_eq!(&repeated[..8], &repeated[8..], "identical blocks encrypt identically");
    }

    #[test]
    fn verification_rejects_a_wrong_response_and_a_wrong_length() {
        let password = VncPassword::new("sesame").unwrap();
        let challenge = generate_challenge();
        let response = password.response(&challenge);

        assert!(password.verify(&challenge, &response));
        assert!(!password.verify(&challenge, &password.response(&generate_challenge())));
        // A one-byte-flip in every position must be caught.
        for i in 0..RESPONSE_LEN {
            let mut wrong = response;
            wrong[i] ^= 0x01;
            assert!(!password.verify(&challenge, &wrong), "bit flip in byte {i} was accepted");
        }
        for short in [0usize, 1, 8, 15] {
            assert!(
                !password.verify(&challenge, &response[..short]),
                "a {short}-byte response must be rejected"
            );
        }
        let mut long = response.to_vec();
        long.push(0);
        assert!(!password.verify(&challenge, &long));
    }

    #[test]
    fn a_different_password_produces_a_different_response() {
        let challenge = generate_challenge();
        let a = VncPassword::response_for("secret1", &challenge).unwrap();
        let b = VncPassword::response_for("secret2", &challenge).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_different_challenge_produces_a_different_response() {
        let password = VncPassword::new("secret1").unwrap();
        let a = password.response(&generate_challenge());
        let b = password.response(&generate_challenge());
        assert_ne!(a, b, "a replayed response must not verify against a fresh challenge");
    }

    #[test]
    fn challenges_are_sixteen_bytes_and_do_not_repeat() {
        let a = generate_challenge();
        let b = generate_challenge();
        assert_eq!(a.len(), CHALLENGE_LEN);
        assert_ne!(a, b);
        // Not all zero, and not a constant: a broken RNG is the CVE-2026-44040
        // failure mode and it should show up here first.
        assert_ne!(a, [0u8; 16]);
        assert!(a.iter().any(|&byte| byte != 0));
    }

    #[test]
    fn the_key_is_never_printable() {
        let password = VncPassword::new("sesame").unwrap();
        let debug = format!("{password:?}");
        assert!(debug.contains("redacted"), "got {debug}");
        assert!(!debug.contains("cea6ce86"), "the key leaked into Debug: {debug}");

        let display = password.to_string();
        assert!(!display.contains("sesame"), "the password leaked into Display: {display}");
        assert!(!display.contains("cea6ce86"), "the key leaked into Display: {display}");
        assert_eq!(display, "VncPassword(<redacted>)");
    }
}
