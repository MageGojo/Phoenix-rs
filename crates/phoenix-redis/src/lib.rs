//! Redis adapters for distributed Phoenix backends.
//!
//! Provides production implementations of:
//! - [`phoenix_security::SessionBackend`]
//! - [`phoenix_security::RateLimitBackend`]
//! - [`phoenix_crypto::TokenStore`]
//!
//! See `docs/REDIS.md` for key space and atomicity rules.

#![forbid(unsafe_code)]

mod keys;
mod rate_limit;
mod session;
mod token;

pub use keys::{
    access_key, family_key, family_members_key, rate_limit_key, redact_redis_url, refresh_key,
    session_key,
};
pub use rate_limit::RedisRateLimitBackend;
pub use session::RedisSessionBackend;
pub use token::RedisTokenStore;

use redis::aio::ConnectionManager;
use thiserror::Error;

/// Shared Redis connection factory for session, rate-limit, and token stores.
#[derive(Clone)]
pub struct RedisStores {
    conn: ConnectionManager,
    redacted_url: String,
}

impl std::fmt::Debug for RedisStores {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisStores")
            .field("url", &self.redacted_url)
            .finish_non_exhaustive()
    }
}

impl RedisStores {
    /// Connect using a Redis URL (`redis://...`).
    ///
    /// # Errors
    ///
    /// Returns an error when the URL is invalid or the initial connection fails.
    pub async fn connect(url: impl AsRef<str>) -> Result<Self, RedisConnectError> {
        let url = url.as_ref();
        let client = redis::Client::open(url).map_err(RedisConnectError::from_redis)?;
        Self::from_client_with_label(client, redact_redis_url(url)).await
    }

    /// Build stores from an existing [`redis::Client`].
    ///
    /// # Errors
    ///
    /// Returns an error when the connection manager cannot be established.
    pub async fn from_client(client: redis::Client) -> Result<Self, RedisConnectError> {
        let label = format!("{:?}", client.get_connection_info().addr);
        Self::from_client_with_label(client, redact_redis_url(&label)).await
    }

    async fn from_client_with_label(
        client: redis::Client,
        redacted_url: String,
    ) -> Result<Self, RedisConnectError> {
        let conn = ConnectionManager::new(client)
            .await
            .map_err(RedisConnectError::from_redis)?;
        Ok(Self { conn, redacted_url })
    }

    /// Session backend sharing this connection pool.
    #[must_use]
    pub fn session(&self) -> RedisSessionBackend {
        RedisSessionBackend::new(self.conn.clone())
    }

    /// Rate-limit backend sharing this connection pool.
    #[must_use]
    pub fn rate_limit(&self) -> RedisRateLimitBackend {
        RedisRateLimitBackend::new(self.conn.clone())
    }

    /// Token store sharing this connection pool.
    #[must_use]
    pub fn token(&self) -> RedisTokenStore {
        RedisTokenStore::new(self.conn.clone())
    }
}

/// Alias matching the design-doc naming in `docs/REDIS.md`.
pub type RedisBackends = RedisStores;

/// Connection / URL errors surfaced before store operations begin.
#[derive(Debug, Error)]
#[error("redis connection failed: {message}")]
pub struct RedisConnectError {
    message: String,
}

impl RedisConnectError {
    #[allow(clippy::needless_pass_by_value)]
    fn from_redis(error: redis::RedisError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_helper_redacts_password_material() {
        assert_eq!(
            redact_redis_url("redis://alice:hunter2@db.internal:6379/3"),
            "redis://alice:***@db.internal:6379/3"
        );
    }

    #[test]
    fn redis_connect_error_display_hides_backend_noise() {
        let error = RedisConnectError {
            message: "connection refused".to_owned(),
        };
        assert!(error.to_string().contains("redis connection failed"));
        assert!(!error.to_string().contains("password"));
    }

    #[test]
    fn error_mapping_preserves_operator_detail() {
        let session = phoenix_security::SessionBackendError("timeout".to_owned());
        assert_eq!(session.0, "timeout");
        let rate = phoenix_security::RateLimitStoreError("down".to_owned());
        assert_eq!(rate.0, "down");
        let token = phoenix_crypto::TokenStoreError::Io(std::io::Error::other("redis boom"));
        assert!(matches!(token, phoenix_crypto::TokenStoreError::Io(_)));
    }
}
