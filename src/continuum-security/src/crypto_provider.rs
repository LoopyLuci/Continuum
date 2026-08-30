use continuum_core::ContinuumError;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

/// Key exchange algorithms supported by Continuum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyExchangeAlgorithm {
    X25519,
    Kyber512,
    Kyber768,
    Kyber1024,
    X25519Kyber768, // Hybrid
}

impl KeyExchangeAlgorithm {
    /// Get the priority of this algorithm (higher = preferred)
    pub fn priority(&self) -> u8 {
        match self {
            Self::X25519Kyber768 => 100,
            Self::Kyber1024 => 90,
            Self::Kyber768 => 80,
            Self::X25519 => 70,
            Self::Kyber512 => 60,
        }
    }

    /// Check if this is a post-quantum algorithm
    pub fn is_post_quantum(&self) -> bool {
        matches!(
            self,
            Self::Kyber512 | Self::Kyber768 | Self::Kyber1024 | Self::X25519Kyber768
        )
    }

    /// Check if this is a hybrid algorithm
    pub fn is_hybrid(&self) -> bool {
        matches!(self, Self::X25519Kyber768)
    }
}

/// Hybrid key exchange combining classical and post-quantum
pub struct HybridKeyExchange {
    pub algorithm: KeyExchangeAlgorithm,
    pub classical_secret: Option<StaticSecret>,
    pub classical_public: Option<PublicKey>,
    pub pq_secret: Option<Vec<u8>>,
    pub pq_public: Option<Vec<u8>>,
}

impl HybridKeyExchange {
    /// Create a new hybrid key exchange with the given algorithm
    pub fn new(algorithm: KeyExchangeAlgorithm) -> Self {
        Self {
            algorithm,
            classical_secret: None,
            classical_public: None,
            pq_secret: None,
            pq_public: None,
        }
    }

    /// Negotiate the best algorithm from a list of peer-supported algorithms
    pub fn negotiate(peer_algorithms: &[KeyExchangeAlgorithm]) -> KeyExchangeAlgorithm {
        let preference = [
            KeyExchangeAlgorithm::X25519Kyber768,
            KeyExchangeAlgorithm::X25519,
            KeyExchangeAlgorithm::Kyber768,
            KeyExchangeAlgorithm::Kyber1024,
            KeyExchangeAlgorithm::Kyber512,
        ];

        for alg in &preference {
            if peer_algorithms.contains(alg) {
                return *alg;
            }
        }

        // Fall back to X25519
        KeyExchangeAlgorithm::X25519
    }
}

/// Cryptographic provider trait for swappable crypto backends
pub trait CryptoProvider: Send + Sync {
    /// Get the name of this provider
    fn name(&self) -> &str;

    /// Get the supported key exchange algorithms
    fn supported_algorithms(&self) -> Vec<KeyExchangeAlgorithm>;

    /// Generate a new keypair for the given algorithm
    fn generate_keypair(
        &self,
        algorithm: KeyExchangeAlgorithm,
    ) -> Result<(Vec<u8>, Vec<u8>), ContinuumError>;

    /// Perform key exchange
    fn key_exchange(
        &self,
        algorithm: KeyExchangeAlgorithm,
        secret_key: &[u8],
        public_key: &[u8],
    ) -> Result<Vec<u8>, ContinuumError>;

    /// Derive a session key from shared secret
    fn derive_session_key(
        &self,
        shared_secret: &[u8],
        salt: &[u8],
        info: &[u8],
    ) -> Result<Vec<u8>, ContinuumError>;
}

/// Standard crypto provider using x25519-dalek
pub struct StandardCryptoProvider;

impl CryptoProvider for StandardCryptoProvider {
    fn name(&self) -> &str {
        "standard"
    }

    fn supported_algorithms(&self) -> Vec<KeyExchangeAlgorithm> {
        vec![
            KeyExchangeAlgorithm::X25519,
            KeyExchangeAlgorithm::X25519Kyber768,
        ]
    }

    fn generate_keypair(
        &self,
        algorithm: KeyExchangeAlgorithm,
    ) -> Result<(Vec<u8>, Vec<u8>), ContinuumError> {
        match algorithm {
            KeyExchangeAlgorithm::X25519 | KeyExchangeAlgorithm::X25519Kyber768 => {
                let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
                let public = PublicKey::from(&secret);
                Ok((secret.to_bytes().to_vec(), public.to_bytes().to_vec()))
            }
            _ => Err(ContinuumError::NotSupported(
                "Algorithm not supported by standard provider".into(),
            )),
        }
    }

    fn key_exchange(
        &self,
        algorithm: KeyExchangeAlgorithm,
        secret_key: &[u8],
        public_key: &[u8],
    ) -> Result<Vec<u8>, ContinuumError> {
        match algorithm {
            KeyExchangeAlgorithm::X25519 | KeyExchangeAlgorithm::X25519Kyber768 => {
                let mut sk_bytes = [0u8; 32];
                sk_bytes.copy_from_slice(secret_key);
                let mut pk_bytes = [0u8; 32];
                pk_bytes.copy_from_slice(public_key);

                let secret = StaticSecret::from(sk_bytes);
                let public = PublicKey::from(pk_bytes);
                let shared = secret.diffie_hellman(&public);

                Ok(shared.to_bytes().to_vec())
            }
            _ => Err(ContinuumError::NotSupported(
                "Algorithm not supported by standard provider".into(),
            )),
        }
    }

    fn derive_session_key(
        &self,
        shared_secret: &[u8],
        salt: &[u8],
        info: &[u8],
    ) -> Result<Vec<u8>, ContinuumError> {
        use hkdf::Hkdf;
        use sha2::Sha256;

        let hk = Hkdf::<Sha256>::new(Some(salt), shared_secret);
        let mut key = vec![0u8; 32];
        hk.expand(info, &mut key)
            .map_err(|e| ContinuumError::Crypto(format!("HKDF expand failed: {}", e)))?;

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_algorithm_priority() {
        assert!(
            KeyExchangeAlgorithm::X25519Kyber768.priority()
                > KeyExchangeAlgorithm::X25519.priority()
        );
        assert!(KeyExchangeAlgorithm::X25519Kyber768.is_post_quantum());
        assert!(KeyExchangeAlgorithm::X25519Kyber768.is_hybrid());
    }

    #[test]
    fn test_negotiate() {
        let peer_algs = vec![KeyExchangeAlgorithm::X25519, KeyExchangeAlgorithm::Kyber768];
        let negotiated = HybridKeyExchange::negotiate(&peer_algs);
        assert_eq!(negotiated, KeyExchangeAlgorithm::X25519);
    }

    #[test]
    fn test_standard_provider() {
        let provider = StandardCryptoProvider;
        assert_eq!(provider.name(), "standard");
        assert!(provider
            .supported_algorithms()
            .contains(&KeyExchangeAlgorithm::X25519));
    }

    #[test]
    fn test_key_exchange_roundtrip() {
        let provider = StandardCryptoProvider;
        let (sk1, pk1) = provider
            .generate_keypair(KeyExchangeAlgorithm::X25519)
            .unwrap();
        let (sk2, pk2) = provider
            .generate_keypair(KeyExchangeAlgorithm::X25519)
            .unwrap();

        let shared1 = provider
            .key_exchange(KeyExchangeAlgorithm::X25519, &sk1, &pk2)
            .unwrap();
        let shared2 = provider
            .key_exchange(KeyExchangeAlgorithm::X25519, &sk2, &pk1)
            .unwrap();

        assert_eq!(shared1, shared2);
    }
}
