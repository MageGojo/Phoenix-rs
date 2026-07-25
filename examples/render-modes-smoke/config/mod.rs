pub use phoenix::config::{AppConfig, AppConfigBuilder, ConfigError, Environment, SecretValue};

/// Load this application's configuration.
///
/// Reads `config/app.toml`, then `.env`, then process environment.
///
/// # Errors
///
/// Returns a source, validation, or production-requirement error.
pub fn load() -> Result<AppConfig, ConfigError> {
    AppConfig::load()
}
