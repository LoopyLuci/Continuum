//! Machine identity system.

use crate::ContinuumResult;
use ed25519_dalek::SigningKey;
use ed25519_dalek::VerifyingKey;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A persistent machine identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineIdentity {
    /// Ed25519 signing key (persistent, generated on first run)
    signing_key_bytes: [u8; 32],
    /// Human-readable display name
    pub display_name: String,
}

impl MachineIdentity {
    /// Create a new identity with a random signing key.
    pub fn new(display_name: String) -> Self {
        let mut rng = rand::thread_rng();
        let mut signing_key_bytes = [0u8; 32];
        rng.fill(&mut signing_key_bytes);
        Self {
            signing_key_bytes,
            display_name,
        }
    }

    /// Load identity from file, or create a new one if it doesn't exist.
    pub fn load_or_create(path: &Path, display_name: String) -> ContinuumResult<Self> {
        if path.exists() {
            let json = std::fs::read_to_string(path)
                .map_err(|e| crate::ContinuumError::Internal(e.to_string()))?;
            serde_json::from_str(&json)
                .map_err(|e| crate::ContinuumError::Internal(e.to_string()))
        } else {
            let identity = Self::new(display_name);
            identity.save(path)?;
            Ok(identity)
        }
    }

    /// Save identity to file.
    pub fn save(&self, path: &Path) -> ContinuumResult<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crate::ContinuumError::Internal(e.to_string()))?;
        std::fs::write(path, json)
            .map_err(|e| crate::ContinuumError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Get the machine ID (first 8 bytes of the public key hash).
    pub fn machine_id(&self) -> [u8; 8] {
        let verifying_key = self.verifying_key();
        let hash = blake3::hash(verifying_key.as_bytes());
        let mut id = [0u8; 8];
        id.copy_from_slice(&hash.as_bytes()[..8]);
        id
    }

    /// Get the machine ID as a hex string.
    pub fn machine_id_hex(&self) -> String {
        hex::encode(self.machine_id())
    }

    /// Get the verifying public key.
    pub fn verifying_key(&self) -> VerifyingKey {
        let signing_key = SigningKey::from_bytes(&self.signing_key_bytes);
        signing_key.verifying_key()
    }

    /// Get the signing key.
    fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.signing_key_bytes)
    }

    /// Sign a message.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer;
        let signing_key = self.signing_key();
        signing_key.sign(message).to_bytes()
    }

    /// Verify a signature.
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> bool {
        use ed25519_dalek::Verifier;
        let verifying_key = self.verifying_key();
        let sig = ed25519_dalek::Signature::from_bytes(signature);
        verifying_key.verify(message, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_machine_id_deterministic() {
        let id1 = MachineIdentity::new("Test".to_string());
        let id2 = MachineIdentity::new("Test".to_string());
        assert_ne!(id1.machine_id(), id2.machine_id());
        assert_eq!(id1.machine_id(), id1.machine_id());
    }

    #[test]
    fn test_sign_and_verify() {
        let identity = MachineIdentity::new("Test".to_string());
        let message = b"hello world";
        let signature = identity.sign(message);
        assert!(identity.verify(message, &signature));
        assert!(!identity.verify(b"wrong message", &signature));
    }

    #[test]
    fn test_machine_id_hex() {
        let identity = MachineIdentity::new("Test".to_string());
        let hex = identity.machine_id_hex();
        assert_eq!(hex.len(), 16);
    }
}
