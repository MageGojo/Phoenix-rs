//! Secure-by-default JWT, blind indexes, authenticated encryption, and password hashing.

mod blind_index;
mod encryption;
mod jwt;
mod password;
mod token;

pub use blind_index::{BlindIndexError, BlindIndexKey, BlindIndexer, MAX_BLIND_INDEX_KEYS};
pub use encryption::{Ciphertext, EncryptionError, EncryptionKey, Encryptor};
pub use jwt::{Jwt, JwtAuth, JwtClaims, JwtConfig, JwtError, JwtKey, JwtManager, JwtRejection};
pub use password::{Password, PasswordError};
pub use token::{
    FileTokenStore, MemoryTokenStore, RefreshRecord, RotateRefresh, StatefulJwtAuth, TokenError,
    TokenPair, TokenService, TokenStore, TokenStoreError,
};
