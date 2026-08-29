use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{anyhow, Result};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::Zeroize;

fn derive_password_key(password: &str) -> [u8; 32] {
    let salt = b"continuum-pake-v1";
    let hk = Hkdf::<Sha256>::new(Some(salt), password.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(b"password-key", &mut key)
        .expect("HKDF expand failed");
    key
}

fn encrypt_public_key(pk: &[u8; 32], password: &str) -> Vec<u8> {
    let key_bytes = derive_password_key(password);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut OsRng, &mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, pk.as_ref())
        .expect("Encryption failed");

    let mut output = Vec::with_capacity(60);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    output
}

fn decrypt_public_key(encrypted: &[u8], password: &str) -> Result<[u8; 32]> {
    if encrypted.len() < 12 {
        anyhow::bail!("Encrypted public key too short");
    }
    let key_bytes = derive_password_key(password);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let plaintext = cipher
        .decrypt(nonce, &encrypted[12..])
        .map_err(|_| anyhow!("Failed to decrypt public key — wrong password?"))?;

    if plaintext.len() != 32 {
        anyhow::bail!("Decrypted public key has wrong length: {}", plaintext.len());
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&plaintext);
    Ok(pk)
}

pub struct PakeClient {
    secret: EphemeralSecret,
    public: PublicKey,
    password: String,
}

impl PakeClient {
    pub fn new(password: &str) -> Self {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self {
            secret,
            public,
            password: password.to_string(),
        }
    }

    pub fn encrypted_public_key(&self) -> Vec<u8> {
        encrypt_public_key(self.public.as_bytes(), &self.password)
    }

    pub fn complete(self, server_encrypted_pk: &[u8]) -> Result<PakeResult> {
        let server_pk_bytes = decrypt_public_key(server_encrypted_pk, &self.password)?;
        let server_pk = PublicKey::from(server_pk_bytes);
        let shared = self.secret.diffie_hellman(&server_pk);
        let shared_bytes = shared.to_bytes();

        let session_key = derive_session_key(&shared_bytes, &self.password);
        let sas = derive_sas(&shared_bytes, &self.password);

        Ok(PakeResult {
            session_key,
            sas,
            shared_secret: shared_bytes,
        })
    }
}

pub struct PakeServer {
    secret: EphemeralSecret,
    public: PublicKey,
    password: String,
}

impl PakeServer {
    pub fn new(password: &str) -> Self {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self {
            secret,
            public,
            password: password.to_string(),
        }
    }

    pub fn encrypted_public_key(&self) -> Vec<u8> {
        encrypt_public_key(self.public.as_bytes(), &self.password)
    }

    pub fn complete(self, client_encrypted_pk: &[u8]) -> Result<PakeResult> {
        let client_pk_bytes = decrypt_public_key(client_encrypted_pk, &self.password)?;
        let client_pk = PublicKey::from(client_pk_bytes);
        let shared = self.secret.diffie_hellman(&client_pk);
        let shared_bytes = shared.to_bytes();

        let session_key = derive_session_key(&shared_bytes, &self.password);
        let sas = derive_sas(&shared_bytes, &self.password);

        Ok(PakeResult {
            session_key,
            sas,
            shared_secret: shared_bytes,
        })
    }
}

pub struct PakeResult {
    pub session_key: [u8; 32],
    pub sas: [String; 2],
    pub shared_secret: [u8; 32],
}

impl Drop for PakeResult {
    fn drop(&mut self) {
        self.session_key.zeroize();
        self.shared_secret.zeroize();
    }
}

fn derive_session_key(dh_output: &[u8; 32], password: &str) -> [u8; 32] {
    let salt = Sha256::digest(password.as_bytes());
    let hk = Hkdf::<Sha256>::new(Some(&salt), dh_output);
    let mut key = [0u8; 32];
    hk.expand(b"continuum-session-key", &mut key)
        .expect("HKDF expand failed");
    key
}

fn derive_sas(dh_output: &[u8; 32], password: &str) -> [String; 2] {
    let salt = Sha256::digest(password.as_bytes());
    let hk = Hkdf::<Sha256>::new(Some(&salt), dh_output);
    let mut sas_bytes = [0u8; 4];
    hk.expand(b"continuum-sas", &mut sas_bytes)
        .expect("HKDF expand failed");

    let word_list = include_str!("wordlist.txt");
    let words: Vec<&str> = word_list.lines().collect();

    let idx1 = (u16::from_be_bytes([sas_bytes[0], sas_bytes[1]]) as usize) % words.len();
    let idx2 = (u16::from_be_bytes([sas_bytes[2], sas_bytes[3]]) as usize) % words.len();

    [words[idx1].to_string(), words[idx2].to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pake_roundtrip() {
        let password = "test123";
        let client = PakeClient::new(password);
        let server = PakeServer::new(password);

        let client_epk = client.encrypted_public_key();
        let server_epk = server.encrypted_public_key();

        let client_result = client.complete(&server_epk).unwrap();
        let server_result = server.complete(&client_epk).unwrap();

        assert_eq!(client_result.session_key, server_result.session_key);
        assert_eq!(client_result.sas, server_result.sas);
    }

    #[test]
    fn test_pake_wrong_password_fails() {
        let client = PakeClient::new("correct");
        let server = PakeServer::new("wrong");

        let _client_epk = client.encrypted_public_key();
        let server_epk = server.encrypted_public_key();

        let result = client.complete(&server_epk);
        assert!(result.is_err());
    }

    #[test]
    fn test_pake_no_password_transmitted() {
        let password = "secret123";
        let client = PakeClient::new(password);
        let server = PakeServer::new(password);

        let client_epk = client.encrypted_public_key();
        let server_epk = server.encrypted_public_key();

        assert!(!std::str::from_utf8(&client_epk)
            .unwrap_or("")
            .contains(password));
        assert!(!std::str::from_utf8(&server_epk)
            .unwrap_or("")
            .contains(password));
    }

    #[test]
    fn test_sas_consistency() {
        let password = "mypassword";
        let client = PakeClient::new(password);
        let server = PakeServer::new(password);

        let client_epk = client.encrypted_public_key();
        let server_epk = server.encrypted_public_key();

        let client_result = client.complete(&server_epk).unwrap();
        let server_result = server.complete(&client_epk).unwrap();

        assert_eq!(client_result.sas[0], server_result.sas[0]);
        assert_eq!(client_result.sas[1], server_result.sas[1]);

        assert!(!client_result.sas[0].is_empty());
        assert!(!client_result.sas[1].is_empty());
    }

    #[test]
    fn test_different_passwords_different_session_keys() {
        let client1 = PakeClient::new("password1");
        let client2 = PakeClient::new("password2");
        let server1 = PakeServer::new("password1");
        let server2 = PakeServer::new("password2");

        let _epk1c = client1.encrypted_public_key();
        let epk1s = server1.encrypted_public_key();
        let _epk2c = client2.encrypted_public_key();
        let epk2s = server2.encrypted_public_key();

        let r1c = client1.complete(&epk1s).unwrap();
        let r2c = client2.complete(&epk2s).unwrap();

        assert_ne!(r1c.session_key, r2c.session_key);
    }
}
