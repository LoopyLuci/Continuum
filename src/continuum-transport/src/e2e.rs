use anyhow::Result;
use continuum_security::double_ratchet::{EncryptedFrame, RatchetState};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub struct E2EEncryptor {
    ratchet: Option<RatchetState>,
    session_key: Option<[u8; 32]>,
    frame_count: u64,
}

impl Drop for E2EEncryptor {
    fn drop(&mut self) {
        if let Some(ref mut key) = self.session_key {
            key.zeroize();
        }
    }
}

impl E2EEncryptor {
    pub fn new() -> Self {
        Self {
            ratchet: None,
            session_key: None,
            frame_count: 0,
        }
    }

    pub fn init_with_secret(&mut self, shared_secret: [u8; 32]) {
        self.session_key = Some(shared_secret);
        tracing::info!("E2E encryption initialized with shared secret");
    }

    pub fn encrypt_frame(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.frame_count += 1;
        let Some(key) = self.session_key else {
            anyhow::bail!("E2E encryption not initialized — refusing to send plaintext");
        };

        if self.ratchet.is_none() {
            self.ratchet = Some(RatchetState::new(key));
        }

        if let Some(ref mut ratchet) = self.ratchet {
            let encrypted = ratchet.encrypt(plaintext)?;
            Ok(serde_json::to_vec(&encrypted)?)
        } else {
            unreachable!()
        }
    }

    pub fn decrypt_frame(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let Some(key) = self.session_key else {
            anyhow::bail!("E2E decryption not initialized");
        };

        if self.ratchet.is_none() {
            self.ratchet = Some(RatchetState::new(key));
        }

        if let Some(ref mut ratchet) = self.ratchet {
            let encrypted: EncryptedFrame = serde_json::from_slice(data)?;
            let plaintext = ratchet.decrypt(&encrypted)?;
            Ok(plaintext)
        } else {
            unreachable!()
        }
    }

    pub fn is_active(&self) -> bool {
        self.session_key.is_some()
    }
}

impl Default for E2EEncryptor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMediaHeader {
    pub magic: [u8; 4],
    pub version: u8,
    pub key_id: [u8; 8],
    pub iv: [u8; 12],
}

impl EncryptedMediaHeader {
    pub const MAGIC: [u8; 4] = *b"CONE";

    pub fn new(key_id: [u8; 8]) -> Self {
        let mut iv = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut iv);
        Self {
            magic: Self::MAGIC,
            version: 1,
            key_id,
            iv,
        }
    }

    pub fn validate(&self) -> bool {
        self.magic == Self::MAGIC && self.version == 1
    }
}
