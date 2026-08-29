use anyhow::{anyhow, Result};
use base64::Engine;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};
use zeroize::Zeroize;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidDocument {
    pub did: String,
    pub public_key: Vec<u8>,
    pub created: String,
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeInit {
    pub did_doc: DidDocument,
    pub ephemeral_public: Vec<u8>,
    pub nonce: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub did_doc: DidDocument,
    pub ephemeral_public: Vec<u8>,
    pub nonce: Vec<u8>,
    pub proof: Vec<u8>,
}

#[allow(dead_code)]
pub struct DidHandshake {
    local_did: DidDocument,
    private_key: StaticSecret,
    ephemeral_secret: Option<EphemeralSecret>,
    init_nonce: Vec<u8>,
    shared_secret: Option<[u8; 32]>,
}

impl Drop for DidHandshake {
    fn drop(&mut self) {
        self.init_nonce.zeroize();
        if let Some(ref mut secret) = self.shared_secret {
            secret.zeroize();
        }
    }
}

impl DidHandshake {
    pub fn new(identity_seed: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(identity_seed);
        let hash = hasher.finalize();

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&hash);
        let private_key = StaticSecret::from(key_bytes);
        let public_key = PublicKey::from(&private_key);

        let did = format!(
            "did:continuum:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.as_bytes())
        );

        let created = chrono::Utc::now().to_rfc3339();

        let mut proof_data = Vec::new();
        proof_data.extend_from_slice(did.as_bytes());
        proof_data.extend_from_slice(public_key.as_bytes());
        proof_data.extend_from_slice(created.as_bytes());

        let did_doc = DidDocument {
            did,
            public_key: public_key.as_bytes().to_vec(),
            created,
            proof: proof_data,
        };

        Self {
            local_did: did_doc,
            private_key,
            ephemeral_secret: None,
            init_nonce: Vec::new(),
            shared_secret: None,
        }
    }

    pub fn initiate(&mut self) -> HandshakeInit {
        let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
        let ephemeral_public = PublicKey::from(&ephemeral_secret);
        self.ephemeral_secret = Some(ephemeral_secret);

        let mut nonce = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut nonce);
        self.init_nonce = nonce.to_vec();

        HandshakeInit {
            did_doc: self.local_did.clone(),
            ephemeral_public: ephemeral_public.as_bytes().to_vec(),
            nonce: nonce.to_vec(),
        }
    }

    pub fn respond(&mut self, init: &HandshakeInit) -> Result<HandshakeResponse> {
        if init.ephemeral_public.len() != 32 {
            anyhow::bail!(
                "Invalid ephemeral public key length: {}",
                init.ephemeral_public.len()
            );
        }
        if init.nonce.len() < 16 {
            anyhow::bail!("Nonce too short: {}", init.nonce.len());
        }
        let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
        let ephemeral_public = PublicKey::from(&ephemeral_secret);

        let their_ephemeral = {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&init.ephemeral_public);
            PublicKey::from(bytes)
        };

        let shared = ephemeral_secret.diffie_hellman(&their_ephemeral);

        let mut nonce = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut nonce);

        let mut hasher = Sha256::new();
        hasher.update(shared.as_bytes());
        hasher.update(&init.nonce);
        hasher.update(nonce);
        hasher.update(self.local_did.public_key.as_slice());
        hasher.update(init.did_doc.public_key.as_slice());
        let result = hasher.finalize();

        let mut secret = [0u8; 32];
        secret.copy_from_slice(&result);
        self.shared_secret = Some(secret);

        let mut proof_data = Vec::new();
        proof_data.extend_from_slice(self.local_did.did.as_bytes());
        proof_data.extend_from_slice(ephemeral_public.as_bytes());
        proof_data.extend_from_slice(&nonce);
        proof_data.extend_from_slice(&result);

        Ok(HandshakeResponse {
            did_doc: self.local_did.clone(),
            ephemeral_public: ephemeral_public.as_bytes().to_vec(),
            nonce: nonce.to_vec(),
            proof: proof_data,
        })
    }

    pub fn finalize(&mut self, response: &HandshakeResponse) -> Result<[u8; 32]> {
        if response.ephemeral_public.len() != 32 {
            anyhow::bail!("Invalid ephemeral public key length");
        }
        let ephemeral_secret = self
            .ephemeral_secret
            .take()
            .ok_or_else(|| anyhow!("No ephemeral secret available. Call initiate() first."))?;

        let their_ephemeral = {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&response.ephemeral_public);
            PublicKey::from(bytes)
        };

        let shared = ephemeral_secret.diffie_hellman(&their_ephemeral);

        let mut hasher = Sha256::new();
        hasher.update(shared.as_bytes());
        hasher.update(&self.init_nonce);
        hasher.update(&response.nonce);
        hasher.update(response.did_doc.public_key.as_slice());
        hasher.update(self.local_did.public_key.as_slice());
        let result = hasher.finalize();

        let mut secret = [0u8; 32];
        secret.copy_from_slice(&result);
        self.shared_secret = Some(secret);

        Ok(secret)
    }

    pub fn shared_secret(&self) -> Option<[u8; 32]> {
        self.shared_secret
    }

    pub fn local_did(&self) -> &DidDocument {
        &self.local_did
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handshake_roundtrip() {
        let mut alice = DidHandshake::new(b"alice-seed");
        let mut bob = DidHandshake::new(b"bob-seed");

        let init = alice.initiate();
        let response = bob.respond(&init).unwrap();
        let alice_secret = alice.finalize(&response).unwrap();
        let bob_secret = bob.shared_secret().unwrap();

        assert_eq!(alice_secret, bob_secret);
    }

    #[test]
    fn test_did_format() {
        let handshake = DidHandshake::new(b"test-seed");
        assert!(handshake.local_did().did.starts_with("did:continuum:"));
    }

    #[test]
    fn test_did_different_seeds_different_identities() {
        let h1 = DidHandshake::new(b"seed-a");
        let h2 = DidHandshake::new(b"seed-b");
        assert_ne!(h1.local_did().did, h2.local_did().did);
    }

    #[test]
    fn test_did_public_key_length() {
        let handshake = DidHandshake::new(b"test-pk");
        assert_eq!(handshake.local_did().public_key.len(), 32);
    }

    #[test]
    fn test_did_document_has_proof() {
        let handshake = DidHandshake::new(b"test-proof");
        assert!(!handshake.local_did().proof.is_empty());
    }

    #[test]
    fn test_did_shared_secret_none_before_handshake() {
        let handshake = DidHandshake::new(b"test-ss");
        assert!(handshake.shared_secret().is_none());
    }

    #[test]
    fn test_handshake_different_seeds_produce_different_secrets() {
        let mut alice = DidHandshake::new(b"alice-x");
        let mut bob = DidHandshake::new(b"bob-x");
        let init = alice.initiate();
        let response = bob.respond(&init).unwrap();
        let secret1 = alice.finalize(&response).unwrap();

        let mut alice2 = DidHandshake::new(b"alice-y");
        let mut bob2 = DidHandshake::new(b"bob-y");
        let init2 = alice2.initiate();
        let response2 = bob2.respond(&init2).unwrap();
        let secret2 = alice2.finalize(&response2).unwrap();

        assert_ne!(secret1, secret2);
    }

    #[test]
    fn test_initiate_returns_ephemeral_key() {
        let mut handshake = DidHandshake::new(b"test-eph");
        let init = handshake.initiate();
        assert_eq!(init.ephemeral_public.len(), 32);
        assert!(init.nonce.len() >= 16);
    }

    #[test]
    fn test_respond_rejects_short_ephemeral_key() {
        let mut bob = DidHandshake::new(b"bob-short");
        let bad_init = HandshakeInit {
            did_doc: DidDocument {
                did: "did:continuum:test".to_string(),
                public_key: vec![0u8; 32],
                created: "2024-01-01".to_string(),
                proof: vec![],
            },
            ephemeral_public: vec![0u8; 16], // too short
            nonce: vec![0u8; 32],
        };
        let result = bob.respond(&bad_init);
        assert!(result.is_err());
    }

    #[test]
    fn test_respond_rejects_short_nonce() {
        let mut bob = DidHandshake::new(b"bob-nonce");
        let bad_init = HandshakeInit {
            did_doc: DidDocument {
                did: "did:continuum:test".to_string(),
                public_key: vec![0u8; 32],
                created: "2024-01-01".to_string(),
                proof: vec![],
            },
            ephemeral_public: vec![0u8; 32],
            nonce: vec![0u8; 8], // too short
        };
        let result = bob.respond(&bad_init);
        assert!(result.is_err());
    }

    #[test]
    fn test_finalize_rejects_short_ephemeral_key() {
        let mut alice = DidHandshake::new(b"alice-fin");
        alice.initiate();
        let bad_response = HandshakeResponse {
            did_doc: DidDocument {
                did: "did:continuum:test".to_string(),
                public_key: vec![0u8; 32],
                created: "2024-01-01".to_string(),
                proof: vec![],
            },
            ephemeral_public: vec![0u8; 16],
            nonce: vec![0u8; 32],
            proof: vec![],
        };
        let result = alice.finalize(&bad_response);
        assert!(result.is_err());
    }
}
