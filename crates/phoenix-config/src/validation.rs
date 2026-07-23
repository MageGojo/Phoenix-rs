use std::{collections::BTreeSet, net::IpAddr};

use http::Uri;

use crate::ConfigError;

pub(super) fn validate_origin(key: &'static str, value: &str) -> Result<String, ConfigError> {
    let uri = value
        .parse::<Uri>()
        .map_err(|_| ConfigError::invalid(key, "expected one absolute HTTP(S) origin"))?;
    let authority = uri.authority().ok_or_else(|| {
        ConfigError::invalid(key, "expected one absolute HTTP(S) origin with a host")
    })?;
    let valid_scheme = matches!(uri.scheme_str(), Some("http" | "https"));
    if !valid_scheme
        || authority.as_str().contains('@')
        || uri.path() != "/"
        || uri.query().is_some()
    {
        return Err(ConfigError::invalid(
            key,
            "expected one HTTP(S) origin without userinfo, path, or query",
        ));
    }
    normalize_host(authority.host()).ok_or_else(|| ConfigError::invalid(key, "host is empty"))
}

pub(super) fn validate_database_url(value: &str) -> Result<(), ConfigError> {
    if value.starts_with("sqlite:")
        || value.starts_with("postgres:")
        || value.starts_with("postgresql:")
        || value.starts_with("mysql:")
    {
        Ok(())
    } else {
        Err(ConfigError::invalid(
            "DATABASE_URL",
            "expected a sqlite, postgres, postgresql, or mysql URL",
        ))
    }
}

pub(super) fn parse_trusted_proxies(value: &str) -> Result<Vec<IpAddr>, ConfigError> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(str::trim)
        .map(|value| {
            value.parse().map_err(|_| {
                ConfigError::invalid(
                    "TRUSTED_PROXIES",
                    "expected comma-separated IP addresses or `none`",
                )
            })
        })
        .collect()
}

pub(super) fn parse_allowed_hosts(value: &str) -> Result<Vec<String>, ConfigError> {
    let mut hosts = BTreeSet::new();
    for value in value.split(',').map(str::trim) {
        if value.is_empty() || value == "*" || value.contains('@') {
            return Err(ConfigError::invalid(
                "ALLOWED_HOSTS",
                "expected explicit comma-separated hosts without wildcards or userinfo",
            ));
        }
        let authority = value.parse::<http::uri::Authority>().map_err(|_| {
            ConfigError::invalid("ALLOWED_HOSTS", "contains an invalid host or authority")
        })?;
        let host = normalize_host(authority.host())
            .ok_or_else(|| ConfigError::invalid("ALLOWED_HOSTS", "contains an empty host"))?;
        hosts.insert(host);
    }
    if hosts.is_empty() {
        return Err(ConfigError::invalid(
            "ALLOWED_HOSTS",
            "expected at least one explicit host",
        ));
    }
    Ok(hosts.into_iter().collect())
}

pub(super) fn default_allowed_hosts(public_host: &str) -> Vec<String> {
    [public_host, "localhost", "127.0.0.1", "[::1]"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn parse_positive_u64(
    key: &'static str,
    value: Option<String>,
    default: u64,
) -> Result<u64, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ConfigError::invalid(key, "expected a positive integer"))
}

fn normalize_host(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_reject_userinfo_paths_and_non_http_schemes() {
        assert!(validate_origin("APP_URL", "https://app.example.test").is_ok());
        assert!(validate_origin("APP_URL", "https://user@app.example.test").is_err());
        assert!(validate_origin("APP_URL", "https://app.example.test/path").is_err());
        assert!(validate_origin("APP_URL", "ftp://app.example.test").is_err());
    }

    #[test]
    fn database_hosts_proxies_and_limits_fail_closed() {
        assert!(validate_database_url("postgresql://database/app").is_ok());
        assert!(validate_database_url("mysql://database/app").is_ok());
        assert!(validate_database_url("redis://database/app").is_err());
        assert!(parse_trusted_proxies("127.0.0.1,::1").is_ok());
        assert!(parse_trusted_proxies("not-an-ip").is_err());
        assert!(parse_allowed_hosts("app.example.test").is_ok());
        assert!(parse_allowed_hosts("*").is_err());
        assert!(parse_positive_u64("RATE_LIMIT_REQUESTS", Some("0".to_owned()), 60).is_err());
    }
}
