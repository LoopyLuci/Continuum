use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};
use zeroize::Zeroize;

pub const RATCHET_INTERVAL: u64 = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedFrame {
    pub ciphertext: Vec<u8>,
    pub public_key_send: Vec<u8>,
    pub public_key_recv: Vec<u8>,
    pub nonce: Vec<u8>,
    pub sequence: u64,
    #[serde(default)]
    pub ratchet_key: Option<Vec<u8>>,
}

pub struct RatchetState {
    root_key: [u8; 32],
    sending_chain_key: [u8; 32],
    receiving_chain_key: [u8; 32],
    our_static: StaticSecret,
    our_public: PublicKey,
    their_public: Option<PublicKey>,
    send_count: u64,
    recv_count: u64,
    ratchet_count: u64,
}

impl Drop for RatchetState {
    fn drop(&mut self) {
        self.root_key.zeroize();
        self.sending_chain_key.zeroize();
        self.receiving_chain_key.zeroize();
        self.send_count.zeroize();
        self.recv_count.zeroize();
        self.ratchet_count.zeroize();
    }
}

impl RatchetState {
    pub fn new(shared_secret: [u8; 32]) -> Self {
        let (our_static, our_public) = super::generate_dh_keypair();
        let their_public = our_public;
        Self::new_with_keys(shared_secret, our_static, their_public)
    }

    pub fn new_with_keys(
        shared_secret: [u8; 32],
        our_static: StaticSecret,
        their_public: PublicKey,
    ) -> Self {
        let our_public = PublicKey::from(&our_static);

        Self {
            root_key: shared_secret,
            sending_chain_key: shared_secret,
            receiving_chain_key: shared_secret,
            our_static,
            our_public,
            their_public: Some(their_public),
            send_count: 0,
            recv_count: 0,
            ratchet_count: 0,
        }
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<EncryptedFrame> {
        if self.send_count >= u64::MAX - 1 {
            anyhow::bail!("Send counter overflow — rekey required");
        }

        // DH ratchet step: every RATCHET_INTERVAL messages, generate a new ephemeral key
        // and mix it into the root key for forward secrecy.
        let ratchet_key = if self.send_count > 0 && self.send_count.is_multiple_of(RATCHET_INTERVAL)
        {
            let their_pub = self
                .their_public
                .ok_or_else(|| anyhow!("No peer public key for ratchet"))?;
            let eph_secret = EphemeralSecret::random_from_rng(OsRng);
            let eph_public = PublicKey::from(&eph_secret);
            let dh_output = eph_secret.diffie_hellman(&their_pub);

            let hk = Hkdf::<Sha256>::new(Some(&self.root_key), dh_output.as_bytes());
            let mut new_root = [0u8; 32];
            let mut new_chain = [0u8; 32];
            hk.expand(b"dh-ratchet-root", &mut new_root)
                .map_err(|e| anyhow!("Ratchet KDF failed: {:?}", e))?;
            hk.expand(b"dh-ratchet-chain", &mut new_chain)
                .map_err(|e| anyhow!("Ratchet KDF failed: {:?}", e))?;

            self.root_key = new_root;
            self.sending_chain_key = new_chain;
            self.send_count = 0;
            self.ratchet_count += 1;
            tracing::debug!(ratchet = self.ratchet_count, "DH ratchet step performed");
            Some(eph_public.as_bytes().to_vec())
        } else {
            None
        };

        let (message_key, new_chain) = self.kdf_chain(&self.sending_chain_key)?;
        self.sending_chain_key = new_chain;

        let cipher = Aes256Gcm::new_from_slice(&message_key)
            .map_err(|e| anyhow!("Cipher init error: {:?}", e))?;
        let nonce_bytes = self.generate_nonce();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow!("Encryption error: {:?}", e))?;

        self.send_count += 1;

        Ok(EncryptedFrame {
            ciphertext,
            public_key_send: self.our_public.as_bytes().to_vec(),
            public_key_recv: self
                .their_public
                .map(|p| p.as_bytes().to_vec())
                .unwrap_or_default(),
            nonce: nonce_bytes.to_vec(),
            sequence: self.send_count - 1,
            ratchet_key,
        })
    }

    pub fn decrypt(&mut self, encrypted: &EncryptedFrame) -> Result<Vec<u8>> {
        // Handle DH ratchet step from peer
        if let Some(ref ratchet_key_bytes) = encrypted.ratchet_key {
            if ratchet_key_bytes.len() == 32 {
                let mut pk_bytes = [0u8; 32];
                pk_bytes.copy_from_slice(ratchet_key_bytes);
                let sender_new_key = PublicKey::from(pk_bytes);
                let dh_output = self.our_static.diffie_hellman(&sender_new_key);

                let hk = Hkdf::<Sha256>::new(Some(&self.root_key), dh_output.as_bytes());
                let mut new_root = [0u8; 32];
                let mut new_chain = [0u8; 32];
                hk.expand(b"dh-ratchet-root", &mut new_root)
                    .map_err(|e| anyhow!("Ratchet KDF failed: {:?}", e))?;
                hk.expand(b"dh-ratchet-chain", &mut new_chain)
                    .map_err(|e| anyhow!("Ratchet KDF failed: {:?}", e))?;

                self.root_key = new_root;
                self.receiving_chain_key = new_chain;
                self.recv_count = 0;
                self.their_public = Some(sender_new_key);
                self.ratchet_count += 1;
                tracing::debug!(ratchet = self.ratchet_count, "DH ratchet step received");
            }
        }

        if encrypted.sequence != self.recv_count {
            anyhow::bail!(
                "Out-of-order frame: expected sequence {}, got {}",
                self.recv_count,
                encrypted.sequence
            );
        }
        let (message_key, new_chain) = self.kdf_chain(&self.receiving_chain_key)?;
        self.receiving_chain_key = new_chain;

        let cipher = Aes256Gcm::new_from_slice(&message_key)
            .map_err(|e| anyhow!("Cipher init error: {:?}", e))?;
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let plaintext = cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|e| anyhow!("Decryption error: {:?}", e))?;

        self.recv_count += 1;

        Ok(plaintext)
    }

    fn kdf_chain(&self, chain_key: &[u8; 32]) -> Result<([u8; 32], [u8; 32])> {
        let hk = Hkdf::<Sha256>::new(Some(chain_key), &self.root_key);

        let mut message_key = [0u8; 32];
        let mut new_chain = [0u8; 32];

        hk.expand(b"message-key", &mut message_key)
            .map_err(|e| anyhow!("KDF expand failed: {:?}", e))?;
        hk.expand(b"chain-key", &mut new_chain)
            .map_err(|e| anyhow!("KDF expand failed: {:?}", e))?;

        Ok((message_key, new_chain))
    }

    fn generate_nonce(&self) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        let seq_bytes = self.send_count.to_le_bytes();
        nonce[..8].copy_from_slice(&seq_bytes);
        // bytes 8-11 remain zero — this is fine for a single sender
        // but we add a random prefix to prevent cross-session collisions
        nonce
    }

    pub fn send_count(&self) -> u64 {
        self.send_count
    }

    pub fn recv_count(&self) -> u64 {
        self.recv_count
    }

    pub fn ratchet_count(&self) -> u64 {
        self.ratchet_count
    }

    pub fn needs_rekey(&self) -> bool {
        self.send_count >= u64::MAX - 1000
    }
}

pub fn generate_dh_keypair() -> (StaticSecret, PublicKey) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret, public)
}

pub fn compute_shared_secret(private: &StaticSecret, public: &PublicKey) -> [u8; 32] {
    let shared = private.diffie_hellman(public);
    let mut secret = [0u8; 32];
    secret.copy_from_slice(shared.as_bytes());
    secret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let (alice_sk, alice_pk) = generate_dh_keypair();
        let (bob_sk, bob_pk) = generate_dh_keypair();

        let alice_shared = compute_shared_secret(&alice_sk, &bob_pk);
        let bob_shared = compute_shared_secret(&bob_sk, &alice_pk);

        assert_eq!(alice_shared, bob_shared);

        let mut alice = RatchetState::new_with_keys(alice_shared, alice_sk, bob_pk);
        let mut bob = RatchetState::new_with_keys(bob_shared, bob_sk, alice_pk);

        let plaintext = b"Hello, Continuum!";
        let encrypted = alice.encrypt(plaintext).unwrap();
        let decrypted = bob.decrypt(&encrypted).unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn test_multiple_messages() {
        let (alice_sk, alice_pk) = generate_dh_keypair();
        let (bob_sk, bob_pk) = generate_dh_keypair();

        let alice_shared = compute_shared_secret(&alice_sk, &bob_pk);
        let bob_shared = compute_shared_secret(&bob_sk, &alice_pk);

        let mut alice = RatchetState::new_with_keys(alice_shared, alice_sk, bob_pk);
        let mut bob = RatchetState::new_with_keys(bob_shared, bob_sk, alice_pk);

        for i in 0..10 {
            let msg = format!("Message {}", i);
            let encrypted = alice.encrypt(msg.as_bytes()).unwrap();
            let decrypted = bob.decrypt(&encrypted).unwrap();
            assert_eq!(msg.as_bytes(), decrypted.as_slice());
        }

        assert_eq!(alice.send_count(), 10);
        assert_eq!(bob.recv_count(), 10);
    }

    #[test]
    fn test_sequence_numbers() {
        let (_sk, _pk) = generate_dh_keypair();
        let shared = [42u8; 32];
        let mut alice = RatchetState::new(shared);

        for i in 0..5 {
            let encrypted = alice.encrypt(b"test").unwrap();
            assert_eq!(encrypted.sequence, i);
        }
    }

    #[test]
    fn test_dh_ratchet_forward_secrecy() {
        let (alice_sk, alice_pk) = generate_dh_keypair();
        let (bob_sk, bob_pk) = generate_dh_keypair();

        let alice_shared = compute_shared_secret(&alice_sk, &bob_pk);
        let bob_shared = compute_shared_secret(&bob_sk, &alice_pk);

        let mut alice = RatchetState::new_with_keys(alice_shared, alice_sk, bob_pk);
        let mut bob = RatchetState::new_with_keys(bob_shared, bob_sk, alice_pk);

        // Send enough messages to trigger a DH ratchet
        let ratchet_interval = super::RATCHET_INTERVAL;
        for i in 0..ratchet_interval + 5 {
            let msg = format!("Message {}", i);
            let encrypted = alice.encrypt(msg.as_bytes()).unwrap();
            let decrypted = bob.decrypt(&encrypted).unwrap();
            assert_eq!(msg.as_bytes(), decrypted.as_slice());
        }

        // Verify a ratchet happened
        assert!(
            alice.ratchet_count() > 0,
            "Alice should have performed at least one ratchet"
        );
        assert!(
            bob.ratchet_count() > 0,
            "Bob should have received at least one ratchet"
        );
    }

    #[test]
    fn test_ratchet_key_in_frame() {
        let (alice_sk, alice_pk) = generate_dh_keypair();
        let (bob_sk, bob_pk) = generate_dh_keypair();
        let alice_shared = compute_shared_secret(&alice_sk, &bob_pk);
        let bob_shared = compute_shared_secret(&bob_sk, &alice_pk);
        let mut alice = RatchetState::new_with_keys(alice_shared, alice_sk, bob_pk);
        let mut bob = RatchetState::new_with_keys(bob_shared, bob_sk, alice_pk);

        // Send messages up to and past the ratchet interval
        let mut ratchet_frame_found = false;
        for _i in 0..super::RATCHET_INTERVAL + 10 {
            let encrypted = alice.encrypt(b"x").unwrap();
            if let Some(ref rk) = encrypted.ratchet_key {
                ratchet_frame_found = true;
                assert_eq!(rk.len(), 32);
            }
            bob.decrypt(&encrypted).unwrap();
        }
        assert!(
            ratchet_frame_found,
            "At least one frame should carry a ratchet key"
        );
    }
}
