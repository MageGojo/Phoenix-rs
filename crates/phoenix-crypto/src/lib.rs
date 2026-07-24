#![forbid(unsafe_code)]

mod blind_index;
mod encryption;
#[cfg(feature = "jwt")]
mod jwt;
#[cfg(feature = "password")]
mod password;
#[cfg(feature = "jwt")]
mod token;

pub use blind_index::{BlindIndexError, BlindIndexKey, BlindIndexer, MAX_BLIND_INDEX_KEYS};
pub use encryption::{Ciphertext, EncryptionError, EncryptionKey, Encryptor};
#[cfg(feature = "jwt")]
pub use jwt::{Jwt, JwtAuth, JwtClaims, JwtConfig, JwtError, JwtKey, JwtManager, JwtRejection};
#[cfg(feature = "password")]
pub use password::{Password, PasswordError};
#[cfg(feature = "jwt")]
pub use token::{
    FileTokenStore, MemoryTokenStore, RefreshRecord, RotateRefresh, StatefulJwtAuth, TokenError,
    TokenPair, TokenService, TokenStore, TokenStoreError,
};
