//! Redis-backed distributed [`ScheduleLock`](phoenix_schedule::ScheduleLock).
//!
//! Acquisition is `SET key token NX PX ttl` — atomic "set if absent with an
//! expiry". Release runs a small Lua script that deletes the key **only if it
//! still holds our token**, so a lock whose TTL already lapsed (and was
//! re-acquired by another instance) is never deleted out from under its new
//! owner. The PX TTL is the backstop if a holder crashes before releasing.

use std::time::Duration;

use phoenix_schedule::{BoxLockFuture, LockGuard, ScheduleLock};
use redis::Script;
use redis::aio::ConnectionManager;
use uuid::Uuid;

use crate::keys::schedule_lock_key;

// KEYS: [lock_key]  ARGV: [token]
const RELEASE_SCRIPT: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

/// Distributed scheduler overlap lock backed by Redis.
///
/// Construct via [`RedisStores::schedule_lock`](crate::RedisStores::schedule_lock)
/// and inject with `Schedule::with_lock`. Only one instance across the fleet
/// runs a given job at a time.
#[derive(Clone)]
pub struct RedisScheduleLock {
    conn: ConnectionManager,
}

impl std::fmt::Debug for RedisScheduleLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisScheduleLock")
            .finish_non_exhaustive()
    }
}

impl RedisScheduleLock {
    pub(crate) fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

impl ScheduleLock for RedisScheduleLock {
    fn try_acquire(&self, name: String, ttl: Duration) -> BoxLockFuture {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let key = schedule_lock_key(&name);
            let token = Uuid::new_v4().to_string();
            let ttl_ms = u64::try_from(ttl.as_millis().max(1)).unwrap_or(u64::MAX);

            let acquired: Option<String> = redis::cmd("SET")
                .arg(&key)
                .arg(&token)
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms)
                .query_async(&mut conn)
                .await
                .unwrap_or_else(|error| {
                    // Fail closed: if Redis is unreachable we cannot prove we
                    // hold the lock, so treat it as held and skip the run.
                    tracing::warn!(job = %name, %error, "schedule lock acquire failed; skipping run");
                    None
                });

            acquired?;

            Some(LockGuard::new(move || release(conn, key, token)))
        })
    }
}

/// Best-effort release: run the compare-and-delete script on the current Tokio
/// runtime. If no runtime is available (guard dropped outside async context)
/// the lock simply expires via its PX TTL.
fn release(conn: ConnectionManager, key: String, token: String) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let mut conn = conn;
        let _: Result<i64, _> = Script::new(RELEASE_SCRIPT)
            .key(&key)
            .arg(&token)
            .invoke_async(&mut conn)
            .await;
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_script_is_owner_guarded() {
        // The release script must compare the stored token before deleting so
        // it never removes a lock re-acquired by another instance.
        assert!(super::RELEASE_SCRIPT.contains("== ARGV[1]"));
        assert!(super::RELEASE_SCRIPT.contains("DEL"));
    }
}
