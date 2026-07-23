//! Redis [`RateLimitBackend`](phoenix_security::RateLimitBackend).

use std::time::Duration;

use phoenix_http::BoxFuture;
use phoenix_security::{RateLimitBackend, RateLimitDecision, RateLimitStoreError};
use redis::Script;
use redis::aio::ConnectionManager;

use crate::keys::rate_limit_key;

const HIT_SCRIPT: &str = r"
local raw = redis.call('GET', KEYS[1])
local now = tonumber(ARGV[3])
local window = tonumber(ARGV[2])
local limit = tonumber(ARGV[1])
local started_at
local count
if raw then
  local data = cjson.decode(raw)
  started_at = tonumber(data.started_at)
  count = tonumber(data.count)
  if now - started_at >= window then
    started_at = now
    count = 0
  end
else
  started_at = now
  count = 0
end
count = count + 1
local elapsed = now - started_at
local remaining_window = window - elapsed
if remaining_window < 1 then
  remaining_window = 1
end
local payload = cjson.encode({started_at = started_at, count = count})
redis.call('SET', KEYS[1], payload, 'EX', remaining_window)
local allowed = 0
if count <= limit then
  allowed = 1
end
local remaining = limit - count
if remaining < 0 then
  remaining = 0
end
return {allowed, remaining, remaining_window}
";

#[derive(Clone)]
pub struct RedisRateLimitBackend {
    conn: ConnectionManager,
}

impl std::fmt::Debug for RedisRateLimitBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisRateLimitBackend")
            .finish_non_exhaustive()
    }
}

impl RedisRateLimitBackend {
    #[must_use]
    pub(crate) fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

impl RateLimitBackend for RedisRateLimitBackend {
    fn hit(
        &self,
        key: String,
        limit: u64,
        window: Duration,
        now: u64,
    ) -> BoxFuture<Result<RateLimitDecision, RateLimitStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let window_secs = window.as_secs().max(1);
            let result: Vec<i64> = Script::new(HIT_SCRIPT)
                .key(rate_limit_key(&key))
                .arg(limit)
                .arg(window_secs)
                .arg(now)
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            if result.len() != 3 {
                return Err(RateLimitStoreError(
                    "rate-limit script returned unexpected arity".to_owned(),
                ));
            }
            Ok(RateLimitDecision {
                allowed: result[0] == 1,
                remaining: u64::try_from(result[1].max(0)).unwrap_or(0),
                retry_after: Duration::from_secs(u64::try_from(result[2].max(1)).unwrap_or(1)),
            })
        })
    }
}

fn map_err(error: impl std::fmt::Display) -> RateLimitStoreError {
    RateLimitStoreError(error.to_string())
}
