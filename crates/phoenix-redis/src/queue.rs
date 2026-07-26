//! Redis-backed durable [`QueueBackend`](phoenix_queue::QueueBackend).
//!
//! Semantics mirror [`phoenix_queue::MemoryQueue`] but survive restarts and are
//! safe to share across many worker processes: every state transition is a
//! single atomic Lua script (Redis executes scripts serially), so two workers
//! never reserve the same job.
//!
//! ## Key space (per queue `name`)
//!
//! | Key | Type | Contents |
//! | --- | --- | --- |
//! | `phoenix:queue:{name}:ready` | ZSET | score = `available_at` (unix secs), member = job id — the delayed/backoff timeline |
//! | `phoenix:queue:{name}:reserved` | ZSET | score = visibility deadline, member = job id — in-flight jobs |
//! | `phoenix:queue:{name}:jobs` | HASH | job id → envelope JSON (membership == "queued or reserved") |
//! | `phoenix:queue:{name}:attempts` | HASH | job id → reserve count |
//! | `phoenix:queue:{name}:idem` | HASH | idempotency key → job id (while in-flight) |
//! | `phoenix:queue:{name}:dead` | LIST | dead-lettered `{"attempts":N,"envelope":…}` records |
//!
//! ## Guarantees
//!
//! - **Delayed jobs**: `reserve` only returns members of the ready set whose
//!   `available_at <= now`, so `JobEnvelope::with_delay` / retry backoff carry
//!   over unchanged.
//! - **Visibility timeout**: a reserved job is invisible until its deadline; if
//!   its worker crashes without ack/fail/dead-letter it is reclaimed to the
//!   ready set (lazily on the next `reserve`, or eagerly via `reclaim_expired`).
//!   This is at-least-once delivery — keep handlers idempotent.
//! - **Idempotency**: while a job with a given key is in-flight, re-pushing the
//!   key returns the original id (`PushResult::Existing`); the key frees on
//!   ack / dead-letter.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_queue::{JobEnvelope, JobId, PushResult, QueueBackend, QueueError};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};
use serde::Deserialize;

use crate::keys::{QueueKeys, unix_now};

/// Default visibility timeout: how long a reserved job stays invisible before
/// it is treated as lost and returned to the ready set. Tune it above the
/// job's longest expected runtime to avoid duplicate processing.
pub const DEFAULT_VISIBILITY_TIMEOUT: Duration = Duration::from_secs(30);

// KEYS: [jobs, ready, idem]  ARGV: [job_id, envelope, available_at, idem_key, has_idem]
const PUSH_SCRIPT: &str = r"
if ARGV[5] == '1' then
  local existing = redis.call('HGET', KEYS[3], ARGV[4])
  if existing then
    if redis.call('HEXISTS', KEYS[1], existing) == 1 then
      return {'existing', existing}
    end
    redis.call('HDEL', KEYS[3], ARGV[4])
  end
  redis.call('HSET', KEYS[3], ARGV[4], ARGV[1])
end
redis.call('HSET', KEYS[1], ARGV[1], ARGV[2])
redis.call('ZADD', KEYS[2], tonumber(ARGV[3]), ARGV[1])
return {'created', ARGV[1]}
";

// KEYS: [ready, reserved, jobs, attempts]  ARGV: [now, visibility_secs]
const RESERVE_SCRIPT: &str = r"
local now = tonumber(ARGV[1])
local vis = tonumber(ARGV[2])

-- 1. Reclaim reserved jobs whose visibility deadline has passed.
local expired = redis.call('ZRANGEBYSCORE', KEYS[2], '-inf', now)
for i = 1, #expired do
  local jid = expired[i]
  redis.call('ZREM', KEYS[2], jid)
  if redis.call('HEXISTS', KEYS[3], jid) == 1 then
    redis.call('ZADD', KEYS[1], now, jid)
  else
    redis.call('HDEL', KEYS[4], jid)
  end
end

-- 2. Pop the next runnable job (available_at <= now).
while true do
  local ready = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', now, 'LIMIT', 0, 1)
  if #ready == 0 then
    return false
  end
  local jid = ready[1]
  redis.call('ZREM', KEYS[1], jid)
  local env = redis.call('HGET', KEYS[3], jid)
  if env then
    local attempts = redis.call('HINCRBY', KEYS[4], jid, 1)
    local deadline
    if vis > 0 then
      deadline = now + vis
    else
      deadline = now + 315360000
    end
    redis.call('ZADD', KEYS[2], deadline, jid)
    return {env, tostring(attempts)}
  else
    redis.call('HDEL', KEYS[4], jid)
  end
end
";

// KEYS: [reserved, jobs, attempts, idem]  ARGV: [job_id]
// The idempotency key is read off the envelope (decode-only, payload is never
// re-encoded) and freed here so a completed job's key is immediately reusable.
const ACK_SCRIPT: &str = r"
local env = redis.call('HGET', KEYS[2], ARGV[1])
if not env then
  return 'notfound'
end
if redis.call('ZSCORE', KEYS[1], ARGV[1]) == false then
  return 'invalid'
end
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('HDEL', KEYS[3], ARGV[1])
local ik = cjson.decode(env).idempotency_key
if ik and ik ~= cjson.null and redis.call('HGET', KEYS[4], ik) == ARGV[1] then
  redis.call('HDEL', KEYS[4], ik)
end
return 'ok'
";

// KEYS: [reserved, ready, jobs]  ARGV: [job_id, available_at]
const FAIL_SCRIPT: &str = r"
if redis.call('HEXISTS', KEYS[3], ARGV[1]) == 0 then
  return 'notfound'
end
if redis.call('ZSCORE', KEYS[1], ARGV[1]) == false then
  return 'invalid'
end
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('ZADD', KEYS[2], tonumber(ARGV[2]), ARGV[1])
return 'ok'
";

// KEYS: [reserved, jobs, attempts, idem, dead]  ARGV: [job_id, idem_key, has_idem]
// The dead record wraps the untouched envelope JSON by string concatenation so
// the opaque payload is never re-encoded (cjson could lose big-int precision).
const DEAD_LETTER_SCRIPT: &str = r#"
local env = redis.call('HGET', KEYS[2], ARGV[1])
if not env then
  return 'notfound'
end
if redis.call('ZSCORE', KEYS[1], ARGV[1]) == false then
  return 'invalid'
end
local attempts = redis.call('HGET', KEYS[3], ARGV[1])
if not attempts then
  attempts = '0'
end
redis.call('RPUSH', KEYS[5], '{"attempts":' .. attempts .. ',"envelope":' .. env .. '}')
redis.call('ZREM', KEYS[1], ARGV[1])
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('HDEL', KEYS[3], ARGV[1])
if ARGV[3] == '1' and redis.call('HGET', KEYS[4], ARGV[2]) == ARGV[1] then
  redis.call('HDEL', KEYS[4], ARGV[2])
end
return 'ok'
"#;

// KEYS: [reserved, ready, jobs, attempts]  ARGV: [now]
const RECLAIM_SCRIPT: &str = r"
local now = tonumber(ARGV[1])
local expired = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', now)
local count = 0
for i = 1, #expired do
  local jid = expired[i]
  redis.call('ZREM', KEYS[1], jid)
  if redis.call('HEXISTS', KEYS[3], jid) == 1 then
    redis.call('ZADD', KEYS[2], now, jid)
    count = count + 1
  else
    redis.call('HDEL', KEYS[4], jid)
  end
end
return count
";

/// Durable Redis job queue backend.
///
/// Construct via [`RedisStores::queue`](crate::RedisStores::queue). Multiple
/// instances pointed at the same `name` share one durable queue.
#[derive(Clone)]
pub struct RedisQueue {
    conn: ConnectionManager,
    name: String,
    visibility_timeout: Duration,
}

impl std::fmt::Debug for RedisQueue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisQueue")
            .field("name", &self.name)
            .field("visibility_timeout", &self.visibility_timeout)
            .finish_non_exhaustive()
    }
}

impl RedisQueue {
    pub(crate) fn new(conn: ConnectionManager, name: String) -> Self {
        Self {
            conn,
            name,
            visibility_timeout: DEFAULT_VISIBILITY_TIMEOUT,
        }
    }

    /// Override the visibility timeout (default [`DEFAULT_VISIBILITY_TIMEOUT`]).
    ///
    /// [`Duration::ZERO`] disables reclamation: reserved jobs stay reserved
    /// until acked / failed / dead-lettered (like the default `MemoryQueue`).
    /// Granularity is one second.
    #[must_use]
    pub const fn with_visibility_timeout(mut self, timeout: Duration) -> Self {
        self.visibility_timeout = timeout;
        self
    }

    /// The queue name this backend operates on.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    fn keys(&self) -> QueueKeys {
        QueueKeys::new(&self.name)
    }

    /// Snapshot of dead-lettered envelopes (oldest first), attempts restored.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Backend`] on a Redis failure or
    /// [`QueueError::Serialize`] when a stored record cannot be decoded.
    pub async fn dead_letters(&self) -> Result<Vec<JobEnvelope>, QueueError> {
        let mut conn = self.conn.clone();
        let raws: Vec<String> = conn
            .lrange(self.keys().dead, 0, -1)
            .await
            .map_err(backend_err)?;
        let mut out = Vec::with_capacity(raws.len());
        for raw in raws {
            let record: DeadRecord = serde_json::from_str(&raw)?;
            let mut envelope: JobEnvelope = serde_json::from_value(record.envelope)?;
            envelope.attempts = record.attempts;
            out.push(envelope);
        }
        Ok(out)
    }
}

#[derive(Deserialize)]
struct DeadRecord {
    attempts: u32,
    envelope: serde_json::Value,
}

impl QueueBackend for RedisQueue {
    async fn push(&self, job: JobEnvelope) -> Result<PushResult, QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let job_id = job.id.to_string();
        let envelope = serde_json::to_string(&job)?;
        let available_at = unix_secs(job.available_at);
        let (idem, has_idem) = match job.idempotency_key.as_deref() {
            Some(key) => (key.to_owned(), "1"),
            None => (String::new(), "0"),
        };

        let result: Vec<String> = Script::new(PUSH_SCRIPT)
            .key(keys.jobs)
            .key(keys.ready)
            .key(keys.idem)
            .arg(&job_id)
            .arg(envelope)
            .arg(available_at)
            .arg(idem)
            .arg(has_idem)
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;

        match result.first().map(String::as_str) {
            Some("created") => Ok(PushResult::Created(job.id)),
            Some("existing") => {
                let id = result.get(1).ok_or_else(|| {
                    QueueError::Backend("push returned no existing id".to_owned())
                })?;
                Ok(PushResult::Existing(parse_job_id(id)?))
            }
            other => Err(QueueError::Backend(format!(
                "unexpected push result: {other:?}"
            ))),
        }
    }

    async fn reserve(&self) -> Result<Option<JobEnvelope>, QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let result: Option<Vec<String>> = Script::new(RESERVE_SCRIPT)
            .key(keys.ready)
            .key(keys.reserved)
            .key(keys.jobs)
            .key(keys.attempts)
            .arg(unix_now())
            .arg(self.visibility_timeout.as_secs())
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;

        let Some(parts) = result else {
            return Ok(None);
        };
        let envelope = parts
            .first()
            .ok_or_else(|| QueueError::Backend("reserve returned empty payload".to_owned()))?;
        let attempts = parts
            .get(1)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let mut job: JobEnvelope = serde_json::from_str(envelope)?;
        job.attempts = attempts;
        Ok(Some(job))
    }

    async fn ack(&self, id: &JobId) -> Result<(), QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let (idem, has_idem) = ("", "0");
        let status: String = Script::new(ACK_SCRIPT)
            .key(keys.reserved)
            .key(keys.jobs)
            .key(keys.attempts)
            .key(keys.idem)
            .arg(id.to_string())
            .arg(idem)
            .arg(has_idem)
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;
        interpret_status(&status, id)
    }

    async fn fail(&self, id: &JobId, available_at: SystemTime) -> Result<(), QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let status: String = Script::new(FAIL_SCRIPT)
            .key(keys.reserved)
            .key(keys.ready)
            .key(keys.jobs)
            .arg(id.to_string())
            .arg(unix_secs(available_at))
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;
        interpret_status(&status, id)
    }

    async fn dead_letter(&self, id: &JobId) -> Result<(), QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let (idem, has_idem) = ("", "0");
        let status: String = Script::new(DEAD_LETTER_SCRIPT)
            .key(keys.reserved)
            .key(keys.jobs)
            .key(keys.attempts)
            .key(keys.idem)
            .key(keys.dead)
            .arg(id.to_string())
            .arg(idem)
            .arg(has_idem)
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;
        interpret_status(&status, id)
    }

    async fn reclaim_expired(&self) -> Result<usize, QueueError> {
        let keys = self.keys();
        let mut conn = self.conn.clone();
        let count: i64 = Script::new(RECLAIM_SCRIPT)
            .key(keys.reserved)
            .key(keys.ready)
            .key(keys.jobs)
            .key(keys.attempts)
            .arg(unix_now())
            .invoke_async(&mut conn)
            .await
            .map_err(backend_err)?;
        Ok(usize::try_from(count.max(0)).unwrap_or(0))
    }
}

fn interpret_status(status: &str, id: &JobId) -> Result<(), QueueError> {
    match status {
        "ok" => Ok(()),
        "notfound" => Err(QueueError::NotFound(*id)),
        "invalid" => Err(QueueError::InvalidState { id: *id }),
        other => Err(QueueError::Backend(format!("unexpected status: {other}"))),
    }
}

fn parse_job_id(raw: &str) -> Result<JobId, QueueError> {
    serde_json::from_value(serde_json::Value::String(raw.to_owned()))
        .map_err(|error| QueueError::Backend(format!("invalid job id `{raw}`: {error}")))
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[allow(clippy::needless_pass_by_value)]
fn backend_err(error: redis::RedisError) -> QueueError {
    QueueError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_record_wrapper_round_trips_attempts_and_payload() {
        // Matches the Lua string built in DEAD_LETTER_SCRIPT.
        let envelope = JobEnvelope::new("email", serde_json::json!({"to": "a@b.c"}), 3, None);
        let envelope_json = serde_json::to_string(&envelope).expect("serialize");
        let wrapper = format!("{{\"attempts\":2,\"envelope\":{envelope_json}}}");

        let record: DeadRecord = serde_json::from_str(&wrapper).expect("decode wrapper");
        assert_eq!(record.attempts, 2);
        let mut restored: JobEnvelope =
            serde_json::from_value(record.envelope).expect("decode envelope");
        restored.attempts = record.attempts;
        assert_eq!(restored.name, "email");
        assert_eq!(restored.attempts, 2);
        assert_eq!(restored.payload, serde_json::json!({"to": "a@b.c"}));
    }

    #[test]
    fn interpret_status_maps_backend_states() {
        let id = JobId::new();
        assert!(interpret_status("ok", &id).is_ok());
        assert!(matches!(
            interpret_status("notfound", &id),
            Err(QueueError::NotFound(_))
        ));
        assert!(matches!(
            interpret_status("invalid", &id),
            Err(QueueError::InvalidState { .. })
        ));
        assert!(matches!(
            interpret_status("weird", &id),
            Err(QueueError::Backend(_))
        ));
    }

    #[test]
    fn parse_job_id_round_trips_through_serde() {
        let id = JobId::new();
        let parsed = parse_job_id(&id.to_string()).expect("parse");
        assert_eq!(parsed, id);
        assert!(parse_job_id("not-a-uuid").is_err());
    }
}
