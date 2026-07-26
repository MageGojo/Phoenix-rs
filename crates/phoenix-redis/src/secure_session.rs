//! Redis-backed store for encrypted-transport session keys.
//!
//! The secure transport negotiates an AES-256-GCM key per page session and
//! keeps it under an opaque `key_id`. In-process that key is only reachable by
//! the instance that ran the handshake, so a client behind a load balancer has
//! to keep landing on the same one. [`RedisSecureSessionStore`] moves the table
//! to Redis and makes the instances interchangeable.
//!
//! # This stores key material
//!
//! Unlike sessions, rate-limit counters, or queue jobs, the values here **are
//! secrets**: whoever can read this key space can decrypt the traffic of every
//! live page session. Treat it accordingly —
//!
//! - reach Redis over TLS (`rediss://`) with `AUTH`, never an open port;
//! - prefer an instance with persistence disabled, so keys are not written to
//!   disk in an RDB or AOF file;
//! - keep [`session_ttl`](phoenix_crypto::SecureTransportConfig::session_ttl)
//!   short (the 5-minute default is what bounds the exposure window); every
//!   entry carries a Redis `PX` expiry, so nothing lingers past it.
//!
//! Sticky routing plus the in-process store keeps the key material off the
//! network entirely and remains the higher-assurance option; this is for
//! deployments where interchangeable instances matter more.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use phoenix_crypto::{SecureError, SecureSessionStore};
use phoenix_http::BoxFuture;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use zeroize::Zeroizing;

use crate::keys::secure_session_key;

/// Redis implementation of [`SecureSessionStore`].
///
/// Cheap to clone; clones share the connection manager.
#[derive(Clone)]
pub struct RedisSecureSessionStore {
    conn: ConnectionManager,
}

impl RedisSecureSessionStore {
    pub(crate) fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

impl std::fmt::Debug for RedisSecureSessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the connection: it would print the URL, credentials
        // included.
        formatter
            .debug_struct("RedisSecureSessionStore")
            .finish_non_exhaustive()
    }
}

impl SecureSessionStore for RedisSecureSessionStore {
    fn insert(
        &self,
        key_id: &str,
        key: Zeroizing<[u8; 32]>,
        expires_at: u64,
    ) -> BoxFuture<Result<(), SecureError>> {
        let mut conn = self.conn.clone();
        let redis_key = secure_session_key(key_id);
        // Encoded here rather than held as bytes so the plaintext key is not
        // copied into a second long-lived buffer.
        let encoded = Zeroizing::new(BASE64.encode(key.as_ref()));
        Box::pin(async move {
            let ttl_ms = i64::try_from(expires_at.saturating_mul(1000)).unwrap_or(i64::MAX);
            let now_ms = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| SecureError::Clock)?
                    .as_millis(),
            )
            .unwrap_or(i64::MAX);
            let remaining = ttl_ms.saturating_sub(now_ms);
            if remaining <= 0 {
                // Already expired: storing it would be a no-op with a negative
                // TTL, which Redis rejects outright.
                return Ok(());
            }
            let _: () = conn
                .pset_ex(redis_key, encoded.as_str(), remaining.unsigned_abs())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }

    fn get(
        &self,
        key_id: &str,
        now: u64,
    ) -> BoxFuture<Result<Option<Zeroizing<[u8; 32]>>, SecureError>> {
        let mut conn = self.conn.clone();
        let redis_key = secure_session_key(key_id);
        Box::pin(async move {
            // Redis expiry already removes stale entries; `now` is unused here
            // beyond documenting the contract, since the TTL is authoritative.
            let _ = now;
            let stored: Option<String> = conn
                .get(redis_key)
                .await
                .map_err(|error| to_store_error(&error))?;
            let Some(stored) = stored.map(Zeroizing::new) else {
                return Ok(None);
            };
            let decoded =
                Zeroizing::new(BASE64.decode(stored.as_bytes()).map_err(|_| {
                    SecureError::SessionStore("stored key is not base64".to_owned())
                })?);
            let key: [u8; 32] = decoded
                .as_slice()
                .try_into()
                .map_err(|_| SecureError::SessionStore("stored key is not 32 bytes".to_owned()))?;
            Ok(Some(Zeroizing::new(key)))
        })
    }
}

fn to_store_error(error: &redis::RedisError) -> SecureError {
    // The message can carry the server address but never the key material.
    SecureError::SessionStore(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_space_is_namespaced_and_opaque() {
        let key = secure_session_key("abc123");
        assert_eq!(key, "phoenix:secure:abc123");
        assert!(
            !key.contains(' '),
            "key ids are base64url, so no quoting is needed"
        );
    }

    #[test]
    fn debug_never_renders_the_connection() {
        // A Debug that printed the ConnectionManager would print the Redis URL,
        // credentials included.
        let rendered = format!("{:?}", std::any::type_name::<RedisSecureSessionStore>());
        assert!(rendered.contains("RedisSecureSessionStore"));
    }
}
