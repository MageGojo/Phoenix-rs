//! Structured application logging with safe, explicit defaults.

use tracing::Subscriber;
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Output format used by the Phoenix tracing subscriber.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LogFormat {
    /// Human-readable, compact output for local development.
    #[default]
    Compact,
    /// Newline-delimited JSON suitable for production log collectors.
    Json,
}

/// Builder for process-wide Phoenix logging.
#[derive(Clone, Debug)]
pub struct Logging {
    format: LogFormat,
    filter: String,
    ansi: bool,
    target: bool,
}

impl Default for Logging {
    fn default() -> Self {
        Self {
            format: LogFormat::Compact,
            filter: "info,hyper=warn".to_owned(),
            ansi: true,
            target: false,
        }
    }
}

impl Logging {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    /// Set the fallback filter used when the configured environment variable is absent.
    #[must_use]
    pub fn filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = filter.into();
        self
    }

    #[must_use]
    pub const fn ansi(mut self, enabled: bool) -> Self {
        self.ansi = enabled;
        self
    }

    #[must_use]
    pub const fn target(mut self, enabled: bool) -> Self {
        self.target = enabled;
        self
    }

    /// Install the subscriber using `PHOENIX_LOG`, falling back to this builder's filter.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid filter or when another global subscriber is installed.
    pub fn init(self) -> Result<LoggingGuard, LoggingError> {
        self.init_with_env("PHOENIX_LOG")
    }

    /// Install the subscriber using a custom environment variable name.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid filter or when another global subscriber is installed.
    pub fn init_with_env(self, variable: &str) -> Result<LoggingGuard, LoggingError> {
        let filter = EnvFilter::try_from_env(variable)
            .or_else(|_| EnvFilter::try_new(&self.filter))
            .map_err(LoggingError::Filter)?;
        match self.format {
            LogFormat::Compact => install(
                Registry::default().with(filter).with(
                    fmt::layer()
                        .compact()
                        .with_ansi(self.ansi)
                        .with_target(self.target),
                ),
            ),
            LogFormat::Json => install(
                Registry::default().with(filter).with(
                    fmt::layer()
                        .json()
                        .with_ansi(false)
                        .with_target(self.target),
                ),
            ),
        }
    }
}

fn install<S>(subscriber: S) -> Result<LoggingGuard, LoggingError>
where
    S: Subscriber + Send + Sync,
{
    subscriber.try_init().map_err(LoggingError::Install)?;
    Ok(LoggingGuard { _private: () })
}

/// Proof that Phoenix installed the process-wide logging subscriber.
#[derive(Debug)]
pub struct LoggingGuard {
    _private: (),
}

#[derive(Debug)]
pub enum LoggingError {
    Filter(tracing_subscriber::filter::ParseError),
    Install(tracing_subscriber::util::TryInitError),
}

impl std::fmt::Display for LoggingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filter(error) => write!(formatter, "invalid logging filter: {error}"),
            Self::Install(error) => write!(formatter, "logging is already initialized: {error}"),
        }
    }
}

impl std::error::Error for LoggingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Filter(error) => Some(error),
            Self::Install(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_and_invalid_fallback_filters_fail() {
        let logging = Logging::new();
        assert_eq!(logging.format, LogFormat::Compact);
        assert_eq!(logging.filter, "info,hyper=warn");

        let error = Logging::new()
            .filter("[invalid")
            .init_with_env("PHOENIX_TEST_LOG_NOT_SET")
            .expect_err("invalid filter must fail");
        assert!(matches!(error, LoggingError::Filter(_)));
    }
}
