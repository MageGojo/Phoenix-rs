//! Layered and validated configuration for Phoenix applications.
//!
//! Values are merged in this order: optional `.env`, process environment,
//! then explicit builder overrides. Production-only requirements are checked
//! before the HTTP server or a management command starts.

use std::{
    collections::BTreeMap, error::Error, fmt, net::IpAddr, str::FromStr, sync::Arc, time::Duration,
};

use zeroize::Zeroizing;

mod builder;
mod files;
mod validation;

pub use builder::AppConfigBuilder;

const DEFAULT_ADDRESS: &str = "127.0.0.1:3000";
const DEFAULT_DATABASE_URL: &str = "sqlite:storage/app.sqlite";
const DEFAULT_VITE_DEV_URL: &str = "http://127.0.0.1:5173";
const DEFAULT_RATE_LIMIT_REQUESTS: u64 = 60;
const DEFAULT_RATE_LIMIT_WINDOW_SECONDS: u64 = 60;
const KNOWN_KEYS: &[&str] = &[
    "ALLOWED_HOSTS",
    "APP_ADDR",
    "APP_ENV",
    "APP_NAME",
    "APP_URL",
    "DATABASE_URL",
    "DB_CONNECTION",
    "DB_PASSWORD",
    "RATE_LIMIT_REQUESTS",
    "RATE_LIMIT_WINDOW_SECONDS",
    "TRUSTED_PROXIES",
    "VITE_DEV_URL",
];

/// Runtime mode controlling local defaults and production validation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Environment {
    #[default]
    Development,
    Test,
    Production,
}

impl Environment {
    #[must_use]
    pub const fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }
}

impl FromStr for Environment {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" | "local" => Ok(Self::Development),
            "test" | "testing" => Ok(Self::Test),
            "production" | "prod" => Ok(Self::Production),
            _ => Err(ConfigError::invalid(
                "APP_ENV",
                "expected development, test, or production",
            )),
        }
    }
}

/// Secret configuration whose debug representation never exposes its value.
#[derive(Clone)]
pub struct SecretValue(Arc<Zeroizing<String>>);

impl SecretValue {
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

/// Validated application settings loaded once during process startup.
#[derive(Clone)]
pub struct AppConfig {
    environment: Environment,
    address: String,
    public_url: String,
    database_url: String,
    vite_dev_url: Option<String>,
    trusted_proxies: Arc<[IpAddr]>,
    allowed_hosts: Arc<[String]>,
    rate_limit_requests: u64,
    rate_limit_window: Duration,
    secrets: Arc<BTreeMap<String, SecretValue>>,
}

impl AppConfig {
    #[must_use]
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    #[must_use]
    pub fn public_url(&self) -> &str {
        &self.public_url
    }

    #[must_use]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    #[must_use]
    pub fn vite_dev_url(&self) -> Option<&str> {
        self.vite_dev_url.as_deref()
    }

    #[must_use]
    pub fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }

    #[must_use]
    pub fn allowed_hosts(&self) -> &[String] {
        &self.allowed_hosts
    }

    #[must_use]
    pub const fn rate_limit_requests(&self) -> u64 {
        self.rate_limit_requests
    }

    #[must_use]
    pub const fn rate_limit_window(&self) -> Duration {
        self.rate_limit_window
    }

    #[must_use]
    pub fn secret(&self, key: &str) -> Option<&SecretValue> {
        self.secrets.get(key)
    }
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("environment", &self.environment)
            .field("address", &self.address)
            .field("public_url", &self.public_url)
            .field("database_url", &"[REDACTED]")
            .field("vite_dev_url", &self.vite_dev_url)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("rate_limit_requests", &self.rate_limit_requests)
            .field("rate_limit_window", &self.rate_limit_window)
            .field("secret_keys", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Stable application configuration failure categories.
#[derive(Debug)]
pub enum ConfigError {
    Dotenv(dotenvy::Error),
    NonUnicode(String),
    MissingProduction(&'static str),
    Invalid {
        key: &'static str,
        reason: &'static str,
    },
    InvalidOwned {
        key: String,
        reason: String,
    },
    InvalidSecretRequirement(String),
    MissingRequiredSecret(String),
    SecretTooShort {
        key: String,
        minimum_bytes: usize,
    },
}

impl ConfigError {
    const fn invalid(key: &'static str, reason: &'static str) -> Self {
        Self::Invalid { key, reason }
    }

    fn invalid_owned(key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidOwned {
            key: key.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dotenv(error) => {
                write!(formatter, "failed to load dotenv configuration: {error}")
            }
            Self::NonUnicode(key) => write!(formatter, "{key} must be valid Unicode"),
            Self::MissingProduction(key) => {
                write!(formatter, "production requires an explicit {key} value")
            }
            Self::Invalid { key, reason } => write!(formatter, "invalid {key}: {reason}"),
            Self::InvalidOwned { key, reason } => write!(formatter, "invalid {key}: {reason}"),
            Self::InvalidSecretRequirement(key) => {
                write!(
                    formatter,
                    "invalid or duplicate required secret name `{key}`"
                )
            }
            Self::MissingRequiredSecret(key) => {
                write!(
                    formatter,
                    "production requires the application secret {key}"
                )
            }
            Self::SecretTooShort { key, minimum_bytes } => {
                write!(
                    formatter,
                    "{key} must contain at least {minimum_bytes} bytes"
                )
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dotenv(error) => Some(error),
            Self::NonUnicode(_)
            | Self::MissingProduction(_)
            | Self::Invalid { .. }
            | Self::InvalidOwned { .. }
            | Self::InvalidSecretRequirement(_)
            | Self::MissingRequiredSecret(_)
            | Self::SecretTooShort { .. } => None,
        }
    }
}
