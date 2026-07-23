use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use zeroize::Zeroizing;

use crate::{
    AppConfig, ConfigError, DEFAULT_ADDRESS, DEFAULT_DATABASE_URL, DEFAULT_RATE_LIMIT_REQUESTS,
    DEFAULT_RATE_LIMIT_WINDOW_SECONDS, DEFAULT_VITE_DEV_URL, Environment, KNOWN_KEYS, SecretValue,
    files::{self, TomlDirectory},
    validation::{
        default_allowed_hosts, parse_allowed_hosts, parse_positive_u64, parse_trusted_proxies,
        validate_database_url, validate_origin,
    },
};

#[derive(Clone, Debug)]
enum ConfigFile {
    Discover,
    Exact(PathBuf),
    Disabled,
}

#[derive(Clone, Debug)]
struct SecretRequirement {
    key: String,
    minimum_bytes: usize,
}

/// Controls configuration-file selection, application secrets, and test overrides.
#[derive(Clone, Debug)]
pub struct AppConfigBuilder {
    config_file: ConfigFile,
    toml_directory: TomlDirectory,
    required_secrets: Vec<SecretRequirement>,
    overrides: BTreeMap<String, String>,
}

impl Default for AppConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AppConfigBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config_file: ConfigFile::Discover,
            toml_directory: TomlDirectory::Discover,
            required_secrets: Vec::new(),
            overrides: BTreeMap::new(),
        }
    }

    /// Load exactly this dotenv file. Unlike conventional discovery, a missing
    /// explicit file is an error.
    #[must_use]
    pub fn config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_file = ConfigFile::Exact(path.into());
        self
    }

    /// Load Laravel-style TOML files from this `config/` directory.
    #[must_use]
    pub fn config_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.toml_directory = TomlDirectory::Exact(path.into());
        self
    }

    /// Disable dotenv and `config/*.toml` loading while retaining process
    /// variables and overrides.
    #[must_use]
    pub fn without_config_file(mut self) -> Self {
        self.config_file = ConfigFile::Disabled;
        self.toml_directory = TomlDirectory::Disabled;
        self
    }

    /// Require a named secret in production and enforce its minimum byte length
    /// whenever supplied. The framework does not infer or reuse keys between
    /// JWT, encryption, blind-index, or other application purposes.
    #[must_use]
    pub fn required_secret(mut self, key: impl Into<String>, minimum_bytes: usize) -> Self {
        self.required_secrets.push(SecretRequirement {
            key: key.into(),
            minimum_bytes,
        });
        self
    }

    /// Add the highest-precedence value, intended for tests and explicit process
    /// bootstrap code.
    #[must_use]
    pub fn override_value(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.overrides.insert(key.into(), value.into());
        self
    }

    /// Merge and validate all configured sources.
    ///
    /// Precedence (low → high): `config/*.toml` → `.env` → process environment →
    /// builder overrides. `DATABASE_URL` from `.env`/process wins over TOML; otherwise
    /// the URL is built from `config/database.toml` + `DB_CONNECTION` / `DB_PASSWORD`.
    ///
    /// # Errors
    ///
    /// Returns a source, validation, or production-requirement error.
    pub fn load(self) -> Result<AppConfig, ConfigError> {
        validate_secret_requirements(&self.required_secrets)?;
        let mut values = files::load_toml_defaults(&self.toml_directory)?;
        let mut explicit_keys = BTreeSet::new();

        match &self.config_file {
            ConfigFile::Discover => match dotenvy::dotenv_iter() {
                Ok(iter) => extend_dotenv(&mut values, &mut explicit_keys, iter)?,
                Err(error) if error.not_found() => {}
                Err(error) => return Err(ConfigError::Dotenv(error)),
            },
            ConfigFile::Exact(path) => {
                let iter = dotenvy::from_path_iter(path).map_err(ConfigError::Dotenv)?;
                extend_dotenv(&mut values, &mut explicit_keys, iter)?;
            }
            ConfigFile::Disabled => {}
        }

        let mut environment_keys = KNOWN_KEYS
            .iter()
            .map(|key| (*key).to_owned())
            .collect::<BTreeSet<_>>();
        environment_keys.extend(
            self.required_secrets
                .iter()
                .map(|requirement| requirement.key.clone()),
        );
        for key in environment_keys {
            match env::var(&key) {
                Ok(value) => {
                    values.insert(key.clone(), value);
                    explicit_keys.insert(key);
                }
                Err(env::VarError::NotPresent) => {}
                Err(env::VarError::NotUnicode(_)) => {
                    return Err(ConfigError::NonUnicode(key));
                }
            }
        }
        for key in self.overrides.keys() {
            explicit_keys.insert(key.clone());
        }
        values.extend(self.overrides);
        files::apply_database_overrides(&mut values, &self.toml_directory, &explicit_keys)?;
        AppConfig::from_values(&values, &self.required_secrets)
    }
}

impl AppConfig {
    /// Load the conventional optional `.env` and process environment.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed files, invalid values, or missing
    /// production requirements.
    pub fn load() -> Result<Self, ConfigError> {
        AppConfigBuilder::new().load()
    }

    #[must_use]
    pub fn builder() -> AppConfigBuilder {
        AppConfigBuilder::new()
    }

    fn from_values(
        values: &BTreeMap<String, String>,
        required_secrets: &[SecretRequirement],
    ) -> Result<Self, ConfigError> {
        let environment = setting(values, "APP_ENV")
            .as_deref()
            .unwrap_or("development")
            .parse()?;
        let address = setting(values, "APP_ADDR").unwrap_or_else(|| DEFAULT_ADDRESS.to_owned());
        if address.is_empty() || address.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(ConfigError::invalid(
                "APP_ADDR",
                "expected a non-empty host:port value",
            ));
        }

        let public_url =
            required_in_production(environment, "APP_URL", setting(values, "APP_URL"), || {
                format!("http://{address}")
            })?;
        let public_host = validate_origin("APP_URL", &public_url)?;

        let database_url = required_in_production(
            environment,
            "DATABASE_URL",
            setting(values, "DATABASE_URL"),
            || DEFAULT_DATABASE_URL.to_owned(),
        )?;
        validate_database_url(&database_url)?;

        let vite_dev_url = if environment.is_production() {
            None
        } else {
            let value =
                setting(values, "VITE_DEV_URL").unwrap_or_else(|| DEFAULT_VITE_DEV_URL.to_owned());
            validate_origin("VITE_DEV_URL", &value)?;
            Some(value)
        };

        let proxies = if environment.is_production() {
            setting(values, "TRUSTED_PROXIES")
                .ok_or(ConfigError::MissingProduction("TRUSTED_PROXIES"))?
        } else {
            setting(values, "TRUSTED_PROXIES").unwrap_or_else(|| "none".to_owned())
        };
        let trusted_proxies = parse_trusted_proxies(&proxies)?;

        let allowed_hosts = match setting(values, "ALLOWED_HOSTS") {
            Some(value) => parse_allowed_hosts(&value)?,
            None if environment.is_production() => {
                return Err(ConfigError::MissingProduction("ALLOWED_HOSTS"));
            }
            None => default_allowed_hosts(&public_host),
        };
        if !allowed_hosts.contains(&public_host) {
            return Err(ConfigError::invalid(
                "ALLOWED_HOSTS",
                "must include the host from APP_URL",
            ));
        }

        let rate_limit_requests = parse_positive_u64(
            "RATE_LIMIT_REQUESTS",
            setting(values, "RATE_LIMIT_REQUESTS"),
            DEFAULT_RATE_LIMIT_REQUESTS,
        )?;
        let rate_limit_window = Duration::from_secs(parse_positive_u64(
            "RATE_LIMIT_WINDOW_SECONDS",
            setting(values, "RATE_LIMIT_WINDOW_SECONDS"),
            DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
        )?);
        let secrets = load_secrets(values, environment, required_secrets)?;

        Ok(Self {
            environment,
            address,
            public_url,
            database_url,
            vite_dev_url,
            trusted_proxies: trusted_proxies.into(),
            allowed_hosts: allowed_hosts.into(),
            rate_limit_requests,
            rate_limit_window,
            secrets: Arc::new(secrets),
        })
    }
}

fn extend_dotenv(
    values: &mut BTreeMap<String, String>,
    explicit_keys: &mut BTreeSet<String>,
    entries: impl IntoIterator<Item = dotenvy::Result<(String, String)>>,
) -> Result<(), ConfigError> {
    for entry in entries {
        let (key, value) = entry.map_err(ConfigError::Dotenv)?;
        values.insert(key.clone(), value);
        explicit_keys.insert(key);
    }
    Ok(())
}

fn setting(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn required_in_production(
    environment: Environment,
    key: &'static str,
    value: Option<String>,
    default: impl FnOnce() -> String,
) -> Result<String, ConfigError> {
    match value {
        Some(value) => Ok(value),
        None if environment.is_production() => Err(ConfigError::MissingProduction(key)),
        None => Ok(default()),
    }
}

fn validate_secret_requirements(requirements: &[SecretRequirement]) -> Result<(), ConfigError> {
    let mut keys = BTreeSet::new();
    for requirement in requirements {
        let valid_key = !requirement.key.is_empty()
            && requirement
                .key
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid_key || requirement.minimum_bytes == 0 || !keys.insert(&requirement.key) {
            return Err(ConfigError::InvalidSecretRequirement(
                requirement.key.clone(),
            ));
        }
    }
    Ok(())
}

fn load_secrets(
    values: &BTreeMap<String, String>,
    environment: Environment,
    requirements: &[SecretRequirement],
) -> Result<BTreeMap<String, SecretValue>, ConfigError> {
    let mut secrets = BTreeMap::new();
    for requirement in requirements {
        match setting(values, &requirement.key) {
            Some(value) if value.len() >= requirement.minimum_bytes => {
                secrets.insert(
                    requirement.key.clone(),
                    SecretValue(Arc::new(Zeroizing::new(value))),
                );
            }
            Some(_) => {
                return Err(ConfigError::SecretTooShort {
                    key: requirement.key.clone(),
                    minimum_bytes: requirement.minimum_bytes,
                });
            }
            None if environment.is_production() => {
                return Err(ConfigError::MissingRequiredSecret(requirement.key.clone()));
            }
            None => {}
        }
    }
    Ok(secrets)
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use super::*;

    fn config(values: &[(&str, &str)]) -> Result<AppConfig, ConfigError> {
        values
            .iter()
            .fold(
                AppConfig::builder().without_config_file(),
                |builder, (key, value)| builder.override_value(*key, *value),
            )
            .load()
    }

    #[test]
    fn development_has_working_local_defaults() {
        let config = config(&[]).unwrap();

        assert_eq!(config.environment(), Environment::Development);
        assert_eq!(config.address(), DEFAULT_ADDRESS);
        assert_eq!(config.database_url(), DEFAULT_DATABASE_URL);
        assert_eq!(config.vite_dev_url(), Some(DEFAULT_VITE_DEV_URL));
        assert!(config.trusted_proxies().is_empty());
        assert!(
            config
                .allowed_hosts()
                .iter()
                .any(|host| host == "localhost")
        );
        assert_eq!(config.rate_limit_requests(), 60);
        assert_eq!(config.rate_limit_window(), Duration::from_mins(1));
    }

    #[test]
    fn production_requires_explicit_network_and_database_boundaries() {
        let missing = config(&[("APP_ENV", "production")]).unwrap_err();
        assert!(matches!(missing, ConfigError::MissingProduction("APP_URL")));

        let config = config(&[
            ("APP_ENV", "production"),
            ("APP_URL", "https://app.example.test"),
            ("DATABASE_URL", "postgresql://database/app"),
            ("TRUSTED_PROXIES", "127.0.0.1,::1"),
            ("ALLOWED_HOSTS", "app.example.test"),
        ])
        .unwrap();
        assert!(config.environment().is_production());
        assert_eq!(config.trusted_proxies().len(), 2);
        assert_eq!(config.allowed_hosts(), ["app.example.test"]);
    }

    #[test]
    fn application_declared_secrets_are_required_and_redacted() {
        let builder = AppConfig::builder()
            .without_config_file()
            .required_secret("JWT_SECRET", 32)
            .override_value("APP_ENV", "production")
            .override_value("APP_URL", "https://app.example.test")
            .override_value("DATABASE_URL", "postgresql://database/app")
            .override_value("TRUSTED_PROXIES", "none")
            .override_value("ALLOWED_HOSTS", "app.example.test");
        assert!(matches!(
            builder.clone().load(),
            Err(ConfigError::MissingRequiredSecret(key)) if key == "JWT_SECRET"
        ));

        let config = builder
            .override_value("JWT_SECRET", "0123456789abcdef0123456789abcdef")
            .load()
            .unwrap();
        assert_eq!(
            config.secret("JWT_SECRET").unwrap().expose(),
            "0123456789abcdef0123456789abcdef"
        );
        assert!(!format!("{config:?}").contains("0123456789abcdef"));
    }

    #[test]
    fn toml_database_connection_selects_pgsql() {
        let id = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("phoenix-config-dir-{}-{id}", std::process::id()));
        let config_dir = root.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("database.toml"),
            r#"
default = "sqlite"

[connections.sqlite]
driver = "sqlite"
database = "storage/app.sqlite"

[connections.pgsql]
driver = "pgsql"
host = "127.0.0.1"
port = 5432
database = "phoenix"
username = "phoenix"
password = ""
"#,
        )
        .unwrap();

        let config = AppConfig::builder()
            .without_config_file()
            .config_directory(&config_dir)
            .override_value("DB_CONNECTION", "pgsql")
            .override_value("DB_PASSWORD", "s3cret")
            .load()
            .unwrap();
        assert_eq!(
            config.database_url(),
            "postgresql://phoenix:s3cret@127.0.0.1:5432/phoenix"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_database_url_wins_over_toml() {
        let id = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("phoenix-config-url-{}-{id}", std::process::id()));
        let config_dir = root.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("database.toml"),
            r#"
default = "pgsql"
[connections.pgsql]
driver = "pgsql"
host = "127.0.0.1"
database = "from-toml"
username = "phoenix"
"#,
        )
        .unwrap();

        let config = AppConfig::builder()
            .without_config_file()
            .config_directory(&config_dir)
            .override_value("DATABASE_URL", "sqlite:storage/override.sqlite")
            .load()
            .unwrap();
        assert_eq!(config.database_url(), "sqlite:storage/override.sqlite");
        fs::remove_dir_all(root).unwrap();
    }
}
