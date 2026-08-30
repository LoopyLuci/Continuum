use crate::*;
///
/// Implement this to add support for new crypto algorithms
/// (post-quantum KEM, hardware-backed keys, etc.).
pub trait CryptoProvider: Send + Sync {
    /// Initialize with a shared secret.
    fn init_with_secret(&mut self, secret: &[u8]);

    /// Check if the provider is initialized and ready.
    fn is_active(&self) -> bool;

    /// Encrypt a message.
    fn encrypt(&mut self, plaintext: &[u8]) -> ContinuumResult<Vec<u8>>;

    /// Decrypt a message.
    fn decrypt(&mut self, ciphertext: &[u8]) -> ContinuumResult<Vec<u8>>;

    /// Get the algorithm identifier for negotiation.
    fn algorithm_id(&self) -> u16;

    /// Get the human-readable algorithm name.
    fn algorithm_name(&self) -> &str;
}

/// E2E encryption trait for frame-level encryption.
pub trait E2EEncryptor: Send + Sync {
    /// Encrypt a frame.
    fn encrypt_frame(&mut self, frame: &[u8]) -> ContinuumResult<Vec<u8>>;

    /// Decrypt a frame.
    fn decrypt_frame(&mut self, frame: &[u8]) -> ContinuumResult<Vec<u8>>;

    /// Check if encryption is active.
    fn is_active(&self) -> bool;

    /// Initialize with a shared secret.
    fn init_with_secret(&mut self, secret: &[u8]);
}
