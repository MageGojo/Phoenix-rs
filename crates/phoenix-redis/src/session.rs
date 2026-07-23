//! Redis [`SessionBackend`](phoenix_security::SessionBackend).

use std::collections::HashMap;

use phoenix_http::BoxFuture;
use phoenix_security::{SessionBackend, SessionBackendError, SessionSnapshot, SessionWrite};
use redis::Script;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::keys::{redis_ttl_secs, session_key};

const LOAD_SCRIPT: &str = r"
local raw = redis.call('GET', KEYS[1])
if not raw then
  return false
end
local data = cjson.decode(raw)
local now = tonumber(ARGV[1])
local refresh = tonumber(ARGV[2])
if tonumber(data.expires_at) <= now then
  redis.call('DEL', KEYS[1])
  return false
end
if refresh > tonumber(data.expires_at) then
  data.expires_at = refresh
  local ttl = tonumber(ARGV[3])
  if ttl < 1 then ttl = 1 end
  redis.call('SET', KEYS[1], cjson.encode(data), 'EX', ttl)
  return cjson.encode(data)
end
return raw
";

const CREATE_SCRIPT: &str = r"
if redis.call('EXISTS', KEYS[1]) == 1 then
  return 'collision'
end
redis.call('SET', KEYS[1], ARGV[1], 'EX', tonumber(ARGV[2]))
return 'saved:1'
";

const SAVE_SCRIPT: &str = r"
local raw = redis.call('GET', KEYS[1])
if not raw then
  return 'missing'
end
local data = cjson.decode(raw)
if tonumber(data.version) ~= tonumber(ARGV[1]) then
  return 'conflict'
end
redis.call('SET', KEYS[1], ARGV[2], 'EX', tonumber(ARGV[3]))
return 'saved:' .. tostring(tonumber(ARGV[1]) + 1)
";

const ROTATE_SCRIPT: &str = r"
if redis.call('EXISTS', KEYS[2]) == 1 then
  return 'collision'
end
local raw = redis.call('GET', KEYS[1])
if not raw then
  return 'missing'
end
local data = cjson.decode(raw)
if tonumber(data.version) ~= tonumber(ARGV[1]) then
  return 'conflict'
end
redis.call('SET', KEYS[2], ARGV[2], 'EX', tonumber(ARGV[3]))
redis.call('DEL', KEYS[1])
return 'saved:' .. tostring(tonumber(ARGV[1]) + 1)
";

const DELETE_SCRIPT: &str = r"
local raw = redis.call('GET', KEYS[1])
if not raw then
  return 'missing'
end
local data = cjson.decode(raw)
if tonumber(data.version) ~= tonumber(ARGV[1]) then
  return 'conflict'
end
redis.call('DEL', KEYS[1])
return 'saved:' .. tostring(tonumber(ARGV[1]) + 1)
";

#[derive(Clone)]
pub struct RedisSessionBackend {
    conn: ConnectionManager,
}

impl std::fmt::Debug for RedisSessionBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisSessionBackend")
            .finish_non_exhaustive()
    }
}

impl RedisSessionBackend {
    #[must_use]
    pub(crate) fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct SessionRecord {
    values: HashMap<String, Value>,
    version: u64,
    expires_at: u64,
}

impl From<SessionRecord> for SessionSnapshot {
    fn from(record: SessionRecord) -> Self {
        Self {
            values: record.values,
            version: record.version,
            expires_at: record.expires_at,
        }
    }
}

impl SessionBackend for RedisSessionBackend {
    fn load(
        &self,
        id: String,
        now: u64,
        refresh_expires_at: u64,
    ) -> BoxFuture<Result<Option<SessionSnapshot>, SessionBackendError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let key = session_key(&id);
            let ttl = redis_ttl_secs(refresh_expires_at.max(now.saturating_add(1)));
            let result: Option<String> = Script::new(LOAD_SCRIPT)
                .key(&key)
                .arg(now)
                .arg(refresh_expires_at)
                .arg(ttl)
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            match result {
                None => Ok(None),
                Some(raw) => {
                    let record: SessionRecord = serde_json::from_str(&raw).map_err(map_err)?;
                    Ok(Some(record.into()))
                }
            }
        })
    }

    fn create(
        &self,
        id: String,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let record = SessionRecord {
                values,
                version: 1,
                expires_at,
            };
            let payload = serde_json::to_string(&record).map_err(map_err)?;
            let status: String = Script::new(CREATE_SCRIPT)
                .key(session_key(&id))
                .arg(payload)
                .arg(redis_ttl_secs(expires_at))
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            parse_write(&status)
        })
    }

    fn save(
        &self,
        id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let version = expected_version.saturating_add(1);
            let record = SessionRecord {
                values,
                version,
                expires_at,
            };
            let payload = serde_json::to_string(&record).map_err(map_err)?;
            let status: String = Script::new(SAVE_SCRIPT)
                .key(session_key(&id))
                .arg(expected_version)
                .arg(payload)
                .arg(redis_ttl_secs(expires_at))
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            parse_write(&status)
        })
    }

    fn rotate(
        &self,
        old_id: String,
        new_id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let version = expected_version.saturating_add(1);
            let record = SessionRecord {
                values,
                version,
                expires_at,
            };
            let payload = serde_json::to_string(&record).map_err(map_err)?;
            let status: String = Script::new(ROTATE_SCRIPT)
                .key(session_key(&old_id))
                .key(session_key(&new_id))
                .arg(expected_version)
                .arg(payload)
                .arg(redis_ttl_secs(expires_at))
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            parse_write(&status)
        })
    }

    fn delete(
        &self,
        id: String,
        expected_version: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let status: String = Script::new(DELETE_SCRIPT)
                .key(session_key(&id))
                .arg(expected_version)
                .invoke_async(&mut conn)
                .await
                .map_err(map_err)?;
            parse_write(&status)
        })
    }
}

fn parse_write(status: &str) -> Result<SessionWrite, SessionBackendError> {
    if let Some(version) = status.strip_prefix("saved:") {
        let version: u64 = version
            .parse()
            .map_err(|_| SessionBackendError("invalid session write status".to_owned()))?;
        return Ok(SessionWrite::Saved { version });
    }
    match status {
        "collision" => Ok(SessionWrite::Collision),
        "conflict" => Ok(SessionWrite::Conflict),
        "missing" => Ok(SessionWrite::Missing),
        other => Err(SessionBackendError(format!(
            "unexpected session write status: {other}"
        ))),
    }
}

fn map_err(error: impl std::fmt::Display) -> SessionBackendError {
    SessionBackendError(error.to_string())
}
