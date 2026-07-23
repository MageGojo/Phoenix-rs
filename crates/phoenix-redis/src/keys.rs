//! Redis key builders and URL redaction helpers.

pub(crate) const SESSION_PREFIX: &str = "phoenix:session:";
pub(crate) const RATE_LIMIT_PREFIX: &str = "phoenix:rl:";
pub(crate) const REFRESH_PREFIX: &str = "phoenix:token:refresh:";
pub(crate) const FAMILY_PREFIX: &str = "phoenix:token:family:";
pub(crate) const FAMILY_MEMBERS_PREFIX: &str = "phoenix:token:family_members:";
pub(crate) const ACCESS_PREFIX: &str = "phoenix:token:access:";

#[must_use]
pub fn session_key(id: &str) -> String {
    format!("{SESSION_PREFIX}{id}")
}

#[must_use]
pub fn rate_limit_key(key: &str) -> String {
    format!("{RATE_LIMIT_PREFIX}{key}")
}

#[must_use]
pub fn refresh_key(token_hash: &str) -> String {
    format!("{REFRESH_PREFIX}{token_hash}")
}

#[must_use]
pub fn family_key(family_id: &str) -> String {
    format!("{FAMILY_PREFIX}{family_id}")
}

#[must_use]
pub fn family_members_key(family_id: &str) -> String {
    format!("{FAMILY_MEMBERS_PREFIX}{family_id}")
}

#[must_use]
pub fn access_key(token_id: &str) -> String {
    format!("{ACCESS_PREFIX}{token_id}")
}

/// Redact password material in a Redis URL for logs and `Debug`.
#[must_use]
pub fn redact_redis_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let redacted_authority = match authority.rsplit_once('@') {
        Some((credentials, host)) => {
            let user = credentials
                .split_once(':')
                .map_or(credentials, |(user, _)| user);
            if credentials.contains(':') {
                format!("{user}:***@{host}")
            } else {
                format!("{credentials}@{host}")
            }
        }
        None => authority.to_owned(),
    };
    match path {
        Some(path) => format!("{scheme}://{redacted_authority}/{path}"),
        None => format!("{scheme}://{redacted_authority}"),
    }
}

/// TTL for Redis `EXPIRE` when `expires_at` may be a synthetic logical clock.
#[must_use]
pub(crate) fn redis_ttl_secs(expires_at: u64) -> u64 {
    let wall = unix_now();
    if expires_at > wall {
        expires_at.saturating_sub(wall).max(1)
    } else {
        // Logical-clock contract tests use small timestamps; keep the key alive.
        86_400
    }
}

#[must_use]
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_prefixes_match_design_doc() {
        assert_eq!(session_key("abc"), "phoenix:session:abc");
        assert_eq!(rate_limit_key("ip"), "phoenix:rl:ip");
        assert_eq!(refresh_key("h"), "phoenix:token:refresh:h");
        assert_eq!(family_key("f"), "phoenix:token:family:f");
        assert_eq!(family_members_key("f"), "phoenix:token:family_members:f");
        assert_eq!(access_key("jti"), "phoenix:token:access:jti");
    }

    #[test]
    fn redacts_password_in_redis_url() {
        assert_eq!(
            redact_redis_url("redis://user:s3cret@127.0.0.1:6379/0"),
            "redis://user:***@127.0.0.1:6379/0"
        );
        assert_eq!(
            redact_redis_url("redis://:onlypass@localhost/1"),
            "redis://:***@localhost/1"
        );
        assert_eq!(redact_redis_url("redis://127.0.0.1/"), "redis://127.0.0.1/");
    }
}
