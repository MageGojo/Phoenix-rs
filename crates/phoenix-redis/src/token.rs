//! Redis [`TokenStore`](phoenix_crypto::TokenStore).

use phoenix_crypto::{RefreshRecord, RotateRefresh, TokenStore, TokenStoreError};
use phoenix_http::BoxFuture;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};

use crate::keys::{
    ACCESS_PREFIX, FAMILY_MEMBERS_PREFIX, FAMILY_PREFIX, REFRESH_PREFIX, access_key, family_key,
    family_members_key, redis_ttl_secs, refresh_key,
};

const ROTATE_SCRIPT: &str = r"
local old_key = ARGV[1]
local new_key = ARGV[2]
local family_prefix = ARGV[3]
local members_prefix = ARGV[4]
local refresh_prefix = ARGV[5]
local replacement_hash = ARGV[6]
local replacement_expires = tonumber(ARGV[7])
local now = tonumber(ARGV[8])
local replacement_ttl = tonumber(ARGV[9])

local raw = redis.call('GET', old_key)
if not raw then
  return {'invalid'}
end
local existing = cjson.decode(raw)
local family_id = existing.family_id
local family_key = family_prefix .. family_id
local members_key = members_prefix .. family_id

local fam = redis.call('GET', family_key)
if fam and tonumber(fam) > now then
  return {'invalid'}
end

if tonumber(existing.expires_at) <= now or existing.revoked == true then
  return {'invalid'}
end

local used = existing.used_at
if used ~= nil and used ~= cjson.null then
  local members = redis.call('SMEMBERS', members_key)
  local max_exp = tonumber(existing.expires_at)
  for _, hash in ipairs(members) do
    local mraw = redis.call('GET', refresh_prefix .. hash)
    if mraw then
      local member = cjson.decode(mraw)
      local exp = tonumber(member.expires_at)
      if exp > max_exp then
        max_exp = exp
      end
    end
  end
  redis.call('SET', family_key, tostring(max_exp), 'EX', math.max(1, max_exp - now))
  for _, hash in ipairs(members) do
    local mkey = refresh_prefix .. hash
    local mraw = redis.call('GET', mkey)
    if mraw then
      local member = cjson.decode(mraw)
      member.revoked = true
      local ttl = redis.call('TTL', mkey)
      if ttl < 1 then ttl = 1 end
      redis.call('SET', mkey, cjson.encode(member), 'EX', ttl)
    end
  end
  return {'reused'}
end

existing.used_at = now
local old_ttl = redis.call('TTL', old_key)
if old_ttl < 1 then old_ttl = 1 end
redis.call('SET', old_key, cjson.encode(existing), 'EX', old_ttl)

local replacement = {
  token_hash = replacement_hash,
  family_id = existing.family_id,
  subject = existing.subject,
  custom = existing.custom,
  expires_at = replacement_expires,
  used_at = cjson.null,
  revoked = false
}
redis.call('SET', new_key, cjson.encode(replacement), 'EX', math.max(1, replacement_ttl))
redis.call('SADD', members_key, replacement_hash)
local members_ttl = redis.call('TTL', members_key)
local needed = math.max(1, replacement_ttl)
if members_ttl < needed then
  redis.call('EXPIRE', members_key, needed)
end
return {'rotated', cjson.encode(replacement)}
";

const REVOKE_FAMILY_SCRIPT: &str = r"
local family_key = ARGV[1]
local members_key = ARGV[2]
local refresh_prefix = ARGV[3]
local expires_at = tonumber(ARGV[4])
local ttl = tonumber(ARGV[5])
local existing = redis.call('GET', family_key)
if existing then
  local current = tonumber(existing)
  if current > expires_at then
    expires_at = current
  end
end
redis.call('SET', family_key, tostring(expires_at), 'EX', math.max(1, ttl))
local members = redis.call('SMEMBERS', members_key)
for _, hash in ipairs(members) do
  local mkey = refresh_prefix .. hash
  local mraw = redis.call('GET', mkey)
  if mraw then
    local member = cjson.decode(mraw)
    member.revoked = true
    local member_ttl = redis.call('TTL', mkey)
    if member_ttl < 1 then member_ttl = 1 end
    redis.call('SET', mkey, cjson.encode(member), 'EX', member_ttl)
  end
end
return 1
";

#[derive(Clone)]
pub struct RedisTokenStore {
    conn: ConnectionManager,
}

impl std::fmt::Debug for RedisTokenStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisTokenStore")
            .finish_non_exhaustive()
    }
}

impl RedisTokenStore {
    #[must_use]
    pub(crate) fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }
}

impl TokenStore for RedisTokenStore {
    fn insert_refresh(&self, record: RefreshRecord) -> BoxFuture<Result<(), TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let key = refresh_key(&record.token_hash);
            let payload = serde_json::to_string(&record)?;
            let set: Option<String> = redis::cmd("SET")
                .arg(&key)
                .arg(&payload)
                .arg("EX")
                .arg(redis_ttl_secs(record.expires_at))
                .arg("NX")
                .query_async(&mut conn)
                .await
                .map_err(map_redis)?;
            if set.is_none() {
                return Err(TokenStoreError::DuplicateToken);
            }
            let members = family_members_key(&record.family_id);
            let _: () = conn
                .sadd(&members, &record.token_hash)
                .await
                .map_err(map_redis)?;
            let ttl = i64::try_from(redis_ttl_secs(record.expires_at)).unwrap_or(1);
            let _: bool = conn.expire(&members, ttl).await.map_err(map_redis)?;
            Ok(())
        })
    }

    fn find_refresh(
        &self,
        token_hash: String,
        now: u64,
    ) -> BoxFuture<Result<Option<RefreshRecord>, TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let raw: Option<String> = conn
                .get(refresh_key(&token_hash))
                .await
                .map_err(map_redis)?;
            let Some(raw) = raw else {
                return Ok(None);
            };
            let record: RefreshRecord = serde_json::from_str(&raw)?;
            if record.expires_at > now && !record.revoked {
                Ok(Some(record))
            } else {
                Ok(None)
            }
        })
    }

    fn rotate_refresh(
        &self,
        token_hash: String,
        replacement_hash: String,
        replacement_expires_at: u64,
        now: u64,
    ) -> BoxFuture<Result<RotateRefresh, TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let result: Vec<String> = Script::new(ROTATE_SCRIPT)
                .arg(refresh_key(&token_hash))
                .arg(refresh_key(&replacement_hash))
                .arg(FAMILY_PREFIX)
                .arg(FAMILY_MEMBERS_PREFIX)
                .arg(REFRESH_PREFIX)
                .arg(&replacement_hash)
                .arg(replacement_expires_at)
                .arg(now)
                .arg(redis_ttl_secs(replacement_expires_at))
                .invoke_async(&mut conn)
                .await
                .map_err(map_redis)?;
            let status = result.first().map_or("invalid", String::as_str);
            match status {
                "invalid" => Ok(RotateRefresh::Invalid),
                "reused" => Ok(RotateRefresh::Reused),
                "rotated" => {
                    let payload = result.get(1).ok_or_else(|| {
                        TokenStoreError::Io(std::io::Error::other(
                            "rotate script missing replacement payload",
                        ))
                    })?;
                    let record: RefreshRecord = serde_json::from_str(payload)?;
                    Ok(RotateRefresh::Rotated(record))
                }
                other => Err(TokenStoreError::Io(std::io::Error::other(format!(
                    "unexpected rotate status: {other}"
                )))),
            }
        })
    }

    fn revoke_family(
        &self,
        family_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let _: i32 = Script::new(REVOKE_FAMILY_SCRIPT)
                .arg(family_key(&family_id))
                .arg(family_members_key(&family_id))
                .arg(REFRESH_PREFIX)
                .arg(expires_at)
                .arg(redis_ttl_secs(expires_at))
                .invoke_async(&mut conn)
                .await
                .map_err(map_redis)?;
            Ok(())
        })
    }

    fn is_family_revoked(
        &self,
        family_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let raw: Option<String> = conn.get(family_key(&family_id)).await.map_err(map_redis)?;
            Ok(raw
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|expires_at| expires_at > now))
        })
    }

    fn revoke_access(
        &self,
        token_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let _: () = conn
                .set_ex(
                    access_key(&token_id),
                    expires_at.to_string(),
                    redis_ttl_secs(expires_at),
                )
                .await
                .map_err(map_redis)?;
            Ok(())
        })
    }

    fn is_access_revoked(
        &self,
        token_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            let raw: Option<String> = conn.get(access_key(&token_id)).await.map_err(map_redis)?;
            Ok(raw
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|expires_at| expires_at > now))
        })
    }

    fn purge_expired(&self, now: u64) -> BoxFuture<Result<(), TokenStoreError>> {
        let mut conn = self.conn.clone();
        Box::pin(async move {
            // Redis TTLs perform most cleanup. Opportunistically drop expired logical keys.
            purge_prefix(&mut conn, REFRESH_PREFIX, now, PurgeKind::Refresh).await?;
            purge_prefix(&mut conn, FAMILY_PREFIX, now, PurgeKind::Timestamp).await?;
            purge_prefix(&mut conn, ACCESS_PREFIX, now, PurgeKind::Timestamp).await?;
            Ok(())
        })
    }
}

enum PurgeKind {
    Refresh,
    Timestamp,
}

async fn purge_prefix(
    conn: &mut ConnectionManager,
    prefix: &str,
    now: u64,
    kind: PurgeKind,
) -> Result<(), TokenStoreError> {
    let pattern = format!("{prefix}*");
    let mut cursor: u64 = 0;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(100)
            .query_async(&mut *conn)
            .await
            .map_err(map_redis)?;
        for key in keys {
            let raw: Option<String> = conn.get(&key).await.map_err(map_redis)?;
            let Some(raw) = raw else {
                continue;
            };
            let expired = match kind {
                PurgeKind::Refresh => {
                    let record: RefreshRecord = serde_json::from_str(&raw)?;
                    record.expires_at <= now
                }
                PurgeKind::Timestamp => raw
                    .parse::<u64>()
                    .map_or(true, |expires_at| expires_at <= now),
            };
            if expired {
                let _: () = conn.del(key).await.map_err(map_redis)?;
            }
        }
        cursor = next;
        if cursor == 0 {
            break;
        }
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn map_redis(error: redis::RedisError) -> TokenStoreError {
    TokenStoreError::Io(std::io::Error::other(error.to_string()))
}
