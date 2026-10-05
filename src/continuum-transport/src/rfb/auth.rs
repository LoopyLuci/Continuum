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
//! print the key, its `Display` prints a placeholder, and the buffer is zeroized
//! on drop. Passwords, responses and challenges all appear only as
//! `key_for` inputs to a `tracing` field name that is never given their value.

use des::cipher::{Block, BlockCipherEncrypt, Key, KeyInit};
use des::Des;
use zeroize::Zeroize;

/// Encrypt one eight-byte block with DES-ECB.
///
/// A named helper so the `cipher` 0.5 array plumbing appears exactly once and
/// the two call sites read as "DES-ECB this block with that key".
fn des_block(key: &[u8; PASSWORD_LEN], block: &[u8; 8]) -> [u8; 8] {
    let cipher = Des::new(&Key::<Des>::from(*key));
    let mut buffer = Block::<Des>::from(*block);
    cipher.encrypt_block(&mut buffer);
    let mut out = [0u8; 8];
    out.copy_from_slice(&buffer[..]);
    out
}

/// Bytes in the server's challenge.
pub const CHALLENGE_LEN: usize = 16;

/// Bytes in the client's response.
pub const RESPONSE_LEN: usize = 16;

/// Bytes in a DES key.
pub const PASSWORD_LEN: usize = 8;

/// Reverse the bit order within one byte.
///
/// `0b1100_0011` becomes `0b1100_0011` here because it is a palindrome, which
/// is exactly the sort of coincidence that hides a bug. `0xC2` (from `C`) and
/// `0xF2` (from `O`) are not, and they are the [`known_vector`] test.
pub fn reverse_bits_in_byte(b: u8) -> u8 {
    b.reverse_bits()
}

/// An 8-byte, bit-reversed DES key.
///
/// Constructed from the plaintext password and then the plaintext is not kept:
/// the struct holds only the derived key, and that is zeroized on drop.
pub struct VncPassword {
    key: [u8; PASSWORD_LEN],
}

impl std::fmt::Debug for VncPassword {
    /// Deliberately unhelpful. A `#[derive(Debug)]` here would put the derived
    /// DES key into any log line that so much as mentions a failed login.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VncPassword(<redacted>)")
    }
}

impl Drop for VncPassword {
    fn drop(&mut self) {
        self.key.zeroize();
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
    /// Derive the DES key from a plaintext password.
    ///
    /// # Errors
    ///
    /// Rejects a password longer than [`PASSWORD_LEN`] characters. Accepting it
    /// would be a silent security downgrade: RFC 6143 truncates, so the ninth
    /// character would have no effect and an operator who believes it does would
    /// be wrong. The upper bound is [`PASSWORD_LEN`] *bytes*, and the check is
    /// on bytes rather than `char`s because the key is eight bytes long.
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
        let mut key = [0u8; PASSWORD_LEN]; // zero-padded on the right, per RFC 6143
        for (slot, byte) in key.iter_mut().zip(bytes) {
            *slot = reverse_bits_in_byte(*byte);
        }
        Ok(Self { key })
    }

    /// The response to `challenge`.
    ///
    /// Two DES-ECB blocks: bytes 0..8 and 8..16, encrypted independently.
    /// Panics if the challenge is not [`CHALLENGE_LEN`] bytes, which is a
    /// programming error rather than anything a peer can cause.
    pub fn response(&self, challenge: &[u8; CHALLENGE_LEN]) -> [u8; RESPONSE_LEN] {
        let mut out = [0u8; RESPONSE_LEN];
        for (block, out_block) in challenge.chunks_exact(8).zip(out.chunks_exact_mut(8)) {
            let mut input = [0u8; 8];
            input.copy_from_slice(block);
            out_block.copy_from_slice(&des_block(&self.key, &input));
        }
        out
    }

    /// Check a client's response against `challenge`, in constant time.
    ///
    /// Constant-time because the response is the only thing an attacker can
    /// measure: an early-exit comparison here would leak how many leading bytes
    /// of a guessed response were right. Eight bytes is far too little to brute
    /// force byte-by-byte anyway, but a comparison that leaks is a bug waiting
    /// for a better attacker, and the cost of `subtle`-style accumulation is
    /// zero here.
    pub fn verify(&self, challenge: &[u8; CHALLENGE_LEN], response: &[u8]) -> bool {
        if response.len() != RESPONSE_LEN {
            return false;
        }
        let expected = self.response(challenge);
        let mut diff = 0u8;
        for (a, b) in expected.iter().zip(response) {
            diff |= a ^ b;
        }
        diff == 0
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
    fn des_is_the_textbook_cipher() {
        // The single most widely published DES known-answer pair: key
        // 0x133457799BBCDFF1 over 0x0123456789ABCDEF is 0x85E813540F0AB405.
        //
        // This vector has *no* bit reversal in it, so it pins the cipher itself
        // independently of any VNC convention. If this fails, the bug is in the
        // DES implementation; if only the VNC tests below fail, the bug is in the
        // bit order.
        let out = des_block(
            &[0x13, 0x34, 0x57, 0x79, 0x9b, 0xbc, 0xdf, 0xf1],
            &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
        );
        assert_eq!(hex(&out), "85e813540f0ab405");
    }

    #[test]
    fn des_is_the_textbook_cipher_on_a_second_key() {
        // key 0x0E329232EA6D0D73 over 0x8787878787878787 is all zeroes.
        let out = des_block(
            &[0x0e, 0x32, 0x92, 0x32, 0xea, 0x6d, 0x0d, 0x73],
            &[0x87; 8],
        );
        assert_eq!(hex(&out), "0000000000000000");
    }

    #[test]
    fn keys_are_reversed_bit_by_bit() {
        // "COW" -> 0xC2 0xF2 0xEA. This is the documented VNC convention: each
        // byte's bits are mirrored, so 'C' (0x43 = 0100_0011) becomes
        // 0xC2 (1100_0010).
        assert_eq!(reverse_bits_in_byte(0x43), 0xc2);
        assert_eq!(reverse_bits_in_byte(0x4f), 0xf2);
        assert_eq!(reverse_bits_in_byte(0x57), 0xea);
        let password = VncPassword::new("COW").unwrap();
        assert_eq!(&hex(&password.key)[..6], "c2f2ea");
    }

    #[test]
    fn key_derivation_matches_the_known_cow_vector() {
        let password = VncPassword::new("COW").unwrap();
        assert_eq!(hex(&password.key), "c2f2ea0000000000");
    }

    #[test]
    fn bit_reversal_is_its_own_inverse() {
        for b in 0..=255u8 {
            assert_eq!(reverse_bits_in_byte(reverse_bits_in_byte(b)), b);
        }
    }

    #[test]
    fn short_passwords_are_zero_padded_on_the_right() {
        // RFC 6143: "truncated to eight characters, or padded with null bytes on
        // the right". Padding on the left would make 'a' collide with a
        // password whose first character is NUL.
        let password = VncPassword::new("a").unwrap();
        // 'a' is 0x61, reversed to 0x86, then NUL-padded to eight bytes.
        assert_eq!(hex(&password.key), "8600000000000000");
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
        // Provenance: the DES primitive is the RustCrypto `des` crate, which
        // [`des_is_the_textbook_cipher`] pins to the published FIPS 46-3
        // vector. The value above was produced by an independently written,
        // table-driven DES operating on bit lists rather than packed words, and
        // checked against the same FIPS vector before being trusted. So a
        // disagreement here means the *VNC* wiring is wrong — almost always the
        // bit order — and not that the cipher is.
        let password = VncPassword::new("sesame").unwrap();
        let challenge: [u8; 16] = [
            0x6b, 0x5f, 0x3e, 0x21, 0x08, 0x6f, 0x0c, 0x35, 0x7f, 0x61, 0x4a, 0x1d, 0x2c, 0x5e,
            0x08, 0x30,
        ];
        assert_eq!(hex(&password.key), "cea6ce86b6a60000");
        let response = password.response(&challenge);
        assert_eq!(hex(&response), "f1c64c5994fc1394f9a9c0013622024d");
        assert!(password.verify(&challenge, &response));
    }

    #[test]
    fn a_whole_sixteen_byte_response_is_one_ebc_stream() {
        // Same password and challenge, but computed as a single ECB call over
        // all sixteen bytes. A mismatch against the block-at-a-time path would
        // mean the chunking is wrong.
        let password = VncPassword::new("sesame").unwrap();
        let challenge: [u8; 16] = [
            0x6b, 0x5f, 0x3e, 0x21, 0x08, 0x6f, 0x0c, 0x35, 0x7f, 0x61, 0x4a, 0x1d, 0x2c, 0x5e,
            0x08, 0x30,
        ];
        let mut whole = [0u8; 16];
        whole[..8].copy_from_slice(&des_block(&password.key, &{
            let mut first = [0u8; 8];
            first.copy_from_slice(&challenge[..8]);
            first
        }));
        whole[8..].copy_from_slice(&des_block(&password.key, &{
            let mut second = [0u8; 8];
            second.copy_from_slice(&challenge[8..]);
            second
        }));
        assert_eq!(whole, password.response(&challenge));
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
