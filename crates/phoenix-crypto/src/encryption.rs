use std::collections::HashMap;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

/// A 256-bit AES-GCM key identified for rotation.
#[derive(Clone)]
pub struct EncryptionKey {
    id: String,
    key: Zeroizing<[u8; 32]>,
}

impl EncryptionKey {
    /// # Errors
    ///
    /// Returns an error for an empty ID or a key that is not exactly 32 bytes.
    pub fn new(id: impl Into<String>, key: impl AsRef<[u8]>) -> Result<Self, EncryptionError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(EncryptionError::InvalidKeyId);
        }
        let key: [u8; 32] = key
            .as_ref()
            .try_into()
            .map_err(|_| EncryptionError::InvalidKeyLength)?;
        Ok(Self {
            id,
            key: Zeroizing::new(key),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EncryptionKey")
            .field("id", &self.id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// Versioned AES-256-GCM output. Authentication data is supplied separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Ciphertext {
    pub version: u8,
    pub algorithm: String,
    pub key_id: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// AES-256-GCM key ring with one active encryption key and rotated decryption keys.
#[derive(Clone, Debug)]
pub struct Encryptor {
    active_key_id: String,
    keys: HashMap<String, EncryptionKey>,
}

impl Encryptor {
    #[must_use]
    pub fn new(active: EncryptionKey) -> Self {
        Self {
            active_key_id: active.id.clone(),
            keys: HashMap::from([(active.id.clone(), active)]),
        }
    }

    #[must_use]
    pub fn with_decryption_key(mut self, key: EncryptionKey) -> Self {
        self.keys.insert(key.id.clone(), key);
        self
    }

    /// Encrypt and authenticate bytes, binding them to caller-supplied context.
    ///
    /// # Errors
    ///
    /// Returns an error when encryption fails.
    pub fn seal(
        &self,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<Ciphertext, EncryptionError> {
        let key = self
            .keys
            .get(&self.active_key_id)
            .ok_or(EncryptionError::UnknownKey)?;
        let cipher = Aes256Gcm::new_from_slice(key.key.as_ref())
            .map_err(|_| EncryptionError::InvalidKeyLength)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| EncryptionError::EncryptionFailed)?;
        Ok(Ciphertext {
            version: 1,
            algorithm: "A256GCM".to_owned(),
            key_id: key.id.clone(),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        })
    }

    /// Decrypt after verifying the key ID, format, tag, and associated context.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed or unauthenticated data.
    pub fn open(
        &self,
        sealed: &Ciphertext,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        if sealed.version != 1 || sealed.algorithm != "A256GCM" {
            return Err(EncryptionError::InvalidEnvelope);
        }
        let key = self
            .keys
            .get(&sealed.key_id)
            .ok_or(EncryptionError::UnknownKey)?;
        let nonce = URL_SAFE_NO_PAD
            .decode(&sealed.nonce)
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&sealed.ciphertext)
            .map_err(|_| EncryptionError::InvalidEnvelope)?;
        let cipher = Aes256Gcm::new_from_slice(key.key.as_ref())
            .map_err(|_| EncryptionError::InvalidKeyLength)?;
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| EncryptionError::AuthenticationFailed)
    }
}

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("encryption key IDs cannot be empty")]
    InvalidKeyId,
    #[error("AES-256-GCM keys must be exactly 32 bytes")]
    InvalidKeyLength,
    #[error("encryption key ID is not recognized")]
    UnknownKey,
    #[error("encrypted data has an unsupported or malformed envelope")]
    InvalidEnvelope,
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("encrypted data failed authentication")]
    AuthenticationFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticates_context_and_supports_decryption_rotation() {
        let old = EncryptionKey::new("old", [11_u8; 32]).unwrap();
        assert!(format!("{old:?}").contains("[REDACTED]"));
        let sealed = Encryptor::new(old.clone())
            .seal(b"private value", b"users:7:email")
            .unwrap();
        let encryptor = Encryptor::new(EncryptionKey::new("current", [12_u8; 32]).unwrap())
            .with_decryption_key(old);

        assert_eq!(
            encryptor.open(&sealed, b"users:7:email").unwrap(),
            b"private value"
        );
        assert!(matches!(
            encryptor.open(&sealed, b"users:8:email"),
            Err(EncryptionError::AuthenticationFailed)
        ));
    }

    #[test]
    fn rejects_bad_keys_envelopes_and_unknown_rotation_ids() {
        assert!(matches!(
            EncryptionKey::new("short", [1_u8; 16]),
            Err(EncryptionError::InvalidKeyLength)
        ));
        let encryptor = Encryptor::new(EncryptionKey::new("active", [2_u8; 32]).unwrap());
        let mut sealed = encryptor.seal(b"payload", b"purpose").unwrap();
        sealed.version = 2;
        assert!(matches!(
            encryptor.open(&sealed, b"purpose"),
            Err(EncryptionError::InvalidEnvelope)
        ));
        sealed.version = 1;
        sealed.key_id = "missing".to_owned();
        assert!(matches!(
            encryptor.open(&sealed, b"purpose"),
            Err(EncryptionError::UnknownKey)
        ));
    }
}
