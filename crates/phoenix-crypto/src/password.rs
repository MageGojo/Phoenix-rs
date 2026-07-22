use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core},
};
use thiserror::Error;

const MAX_PASSWORD_BYTES: usize = 1024;

/// Argon2id password hashing and verification.
#[derive(Clone, Copy, Debug, Default)]
pub struct Password;

impl Password {
    /// Hash a password to a self-describing PHC string with a random salt.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized passwords or hashing failures.
    pub fn hash(password: impl AsRef<[u8]>) -> Result<String, PasswordError> {
        let password = password.as_ref();
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(PasswordError::TooLong);
        }
        let salt = SaltString::generate(&mut rand_core::OsRng);
        Argon2::default()
            .hash_password(password, &salt)
            .map(|hash| hash.to_string())
            .map_err(PasswordError::Hash)
    }

    /// Verify a password against an Argon2 PHC string.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized passwords or malformed stored hashes.
    pub fn verify(password: impl AsRef<[u8]>, encoded: &str) -> Result<bool, PasswordError> {
        let password = password.as_ref();
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(PasswordError::TooLong);
        }
        let parsed = PasswordHash::new(encoded).map_err(PasswordError::Hash)?;
        Ok(Argon2::default().verify_password(password, &parsed).is_ok())
    }
}

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("password exceeds the 1024-byte safety limit")]
    TooLong,
    #[error("password hashing failed")]
    Hash(argon2::password_hash::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2id_hashes_are_salted_and_verifiable() {
        let first = Password::hash("correct horse battery staple").unwrap();
        let second = Password::hash("correct horse battery staple").unwrap();
        assert!(first.starts_with("$argon2id$v=19$"));
        assert_ne!(first, second);
        assert!(Password::verify("correct horse battery staple", &first).unwrap());
        assert!(!Password::verify("wrong", &first).unwrap());
    }

    #[test]
    fn malformed_hashes_and_oversized_passwords_are_rejected() {
        assert!(matches!(
            Password::verify("password", "not-a-phc-hash"),
            Err(PasswordError::Hash(_))
        ));
        assert!(matches!(
            Password::hash(vec![b'x'; MAX_PASSWORD_BYTES + 1]),
            Err(PasswordError::TooLong)
        ));
    }
}
