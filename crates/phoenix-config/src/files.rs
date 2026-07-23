//! Laravel-style TOML configuration files under `config/`.
//!
//! Precedence (low → high): TOML defaults → `.env` → process environment →
//! builder overrides. Secrets should stay in `.env`, not committed TOML.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::ConfigError;

#[derive(Clone, Debug, Default)]
pub(super) enum TomlDirectory {
    #[default]
    Discover,
    Exact(PathBuf),
    Disabled,
}

#[derive(Debug, Deserialize)]
struct AppFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    addr: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DatabaseFile {
    #[serde(default = "default_connection_name")]
    default: String,
    #[serde(default)]
    connections: BTreeMap<String, DatabaseConnection>,
}

fn default_connection_name() -> String {
    "sqlite".to_owned()
}

#[derive(Debug, Deserialize)]
struct DatabaseConnection {
    driver: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

pub(super) fn load_toml_defaults(
    directory: &TomlDirectory,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let Some(root) = resolve_directory(directory)? else {
        return Ok(BTreeMap::new());
    };
    let mut values = BTreeMap::new();
    if let Some(app) = read_optional_toml::<AppFile>(&root.join("app.toml"))? {
        if let Some(name) = app.name.filter(|value| !value.trim().is_empty()) {
            values.insert("APP_NAME".to_owned(), name);
        }
        if let Some(env) = app.env.filter(|value| !value.trim().is_empty()) {
            values.insert("APP_ENV".to_owned(), env);
        }
        if let Some(addr) = app.addr.filter(|value| !value.trim().is_empty()) {
            values.insert("APP_ADDR".to_owned(), addr);
        }
        if let Some(url) = app.url.filter(|value| !value.trim().is_empty()) {
            values.insert("APP_URL".to_owned(), url);
        }
    }
    if let Some(database) = read_optional_toml::<DatabaseFile>(&root.join("database.toml"))? {
        values.insert("DB_CONNECTION".to_owned(), database.default.clone());
        let url = resolve_database_url(&database, None)?;
        values.insert("DATABASE_URL".to_owned(), url);
    }
    Ok(values)
}

/// Resolve the active database URL from TOML + optional connection override.
fn resolve_database_url(
    file: &DatabaseFile,
    connection_override: Option<&str>,
) -> Result<String, ConfigError> {
    let name = connection_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(file.default.as_str());
    let connection = file.connections.get(name).ok_or_else(|| {
        ConfigError::invalid_owned(
            "DB_CONNECTION",
            format!("unknown database connection `{name}` in config/database.toml"),
        )
    })?;
    connection_url(connection)
}

fn connection_url(connection: &DatabaseConnection) -> Result<String, ConfigError> {
    if let Some(url) = connection
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(url.to_owned());
    }
    match connection.driver.trim().to_ascii_lowercase().as_str() {
        "sqlite" => {
            let database = connection
                .database
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("storage/app.sqlite");
            if database == ":memory:" || database.starts_with("sqlite:") {
                Ok(if database.starts_with("sqlite:") {
                    database.to_owned()
                } else {
                    format!("sqlite:{database}")
                })
            } else {
                Ok(format!("sqlite:{database}"))
            }
        }
        "pgsql" | "postgres" | "postgresql" => {
            let host = connection
                .host
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("127.0.0.1");
            let port = connection.port.unwrap_or(5432);
            let database = connection
                .database
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("phoenix");
            let username = connection
                .username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("phoenix");
            let password = connection.password.as_deref().unwrap_or("");
            let auth = if password.is_empty() {
                username.to_owned()
            } else {
                format!("{username}:{}", urlencoding_minimal(password))
            };
            Ok(format!("postgresql://{auth}@{host}:{port}/{database}"))
        }
        "mysql" => {
            let host = connection
                .host
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("127.0.0.1");
            let port = connection.port.unwrap_or(3306);
            let database = connection
                .database
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("phoenix");
            let username = connection
                .username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("phoenix");
            let password = connection.password.as_deref().unwrap_or("");
            let auth = if password.is_empty() {
                username.to_owned()
            } else {
                format!("{username}:{}", urlencoding_minimal(password))
            };
            Ok(format!("mysql://{auth}@{host}:{port}/{database}"))
        }
        other => Err(ConfigError::invalid_owned(
            "DB_CONNECTION",
            format!("unsupported database driver `{other}` (expected sqlite, pgsql, or mysql)"),
        )),
    }
}

fn urlencoding_minimal(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(char::from(byte));
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                output.push('%');
                output.push(char::from(HEX[(byte >> 4) as usize]));
                output.push(char::from(HEX[(byte & 0xf) as usize]));
            }
        }
    }
    output
}

fn resolve_directory(directory: &TomlDirectory) -> Result<Option<PathBuf>, ConfigError> {
    match directory {
        TomlDirectory::Disabled => Ok(None),
        TomlDirectory::Exact(path) => {
            if path.is_dir() {
                Ok(Some(path.clone()))
            } else {
                Err(ConfigError::invalid_owned(
                    "config",
                    format!("config directory not found: {}", path.display()),
                ))
            }
        }
        TomlDirectory::Discover => {
            let path = std::env::current_dir()
                .map_err(|error| {
                    ConfigError::invalid_owned(
                        "config",
                        format!("cannot read current directory: {error}"),
                    )
                })?
                .join("config");
            if path.is_dir() {
                Ok(Some(path))
            } else {
                Ok(None)
            }
        }
    }
}

fn read_optional_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, ConfigError> {
    if !path.is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(path).map_err(|error| {
        ConfigError::invalid_owned(
            "config",
            format!("failed to read {}: {error}", path.display()),
        )
    })?;
    toml::from_str(&source).map(Some).map_err(|error| {
        ConfigError::invalid_owned(
            "config",
            format!("invalid TOML in {}: {error}", path.display()),
        )
    })
}

/// Re-parse database.toml after env merge so `DB_CONNECTION` / `DB_PASSWORD` win.
pub(super) fn apply_database_overrides(
    values: &mut BTreeMap<String, String>,
    directory: &TomlDirectory,
    explicit_keys: &std::collections::BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(root) = resolve_directory(directory)? else {
        return Ok(());
    };
    let Some(mut file) = read_optional_toml::<DatabaseFile>(&root.join("database.toml"))? else {
        return Ok(());
    };
    if let Some(password) = values
        .get("DB_PASSWORD")
        .cloned()
        .filter(|value| !value.trim().is_empty())
    {
        for connection in file.connections.values_mut() {
            if matches!(
                connection.driver.trim().to_ascii_lowercase().as_str(),
                "pgsql" | "postgres" | "postgresql" | "mysql"
            ) {
                connection.password = Some(password.clone());
            }
        }
    }
    if explicit_keys.contains("DATABASE_URL") {
        return Ok(());
    }
    let connection = values
        .get("DB_CONNECTION")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty());
    let url = resolve_database_url(&file, connection)?;
    values.insert("DATABASE_URL".to_owned(), url);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_connection_builds_file_url() {
        let file = DatabaseFile {
            default: "sqlite".to_owned(),
            connections: BTreeMap::from([(
                "sqlite".to_owned(),
                DatabaseConnection {
                    driver: "sqlite".to_owned(),
                    url: None,
                    database: Some("storage/app.sqlite".to_owned()),
                    host: None,
                    port: None,
                    username: None,
                    password: None,
                },
            )]),
        };
        assert_eq!(
            resolve_database_url(&file, None).unwrap(),
            "sqlite:storage/app.sqlite"
        );
    }

    #[test]
    fn pgsql_connection_builds_url_with_encoded_password() {
        let file = DatabaseFile {
            default: "pgsql".to_owned(),
            connections: BTreeMap::from([(
                "pgsql".to_owned(),
                DatabaseConnection {
                    driver: "pgsql".to_owned(),
                    url: None,
                    database: Some("app".to_owned()),
                    host: Some("127.0.0.1".to_owned()),
                    port: Some(5432),
                    username: Some("phoenix".to_owned()),
                    password: Some("p@ss".to_owned()),
                },
            )]),
        };
        assert_eq!(
            resolve_database_url(&file, Some("pgsql")).unwrap(),
            "postgresql://phoenix:p%40ss@127.0.0.1:5432/app"
        );
    }

    #[test]
    fn mysql_connection_builds_url() {
        let file = DatabaseFile {
            default: "mysql".to_owned(),
            connections: BTreeMap::from([(
                "mysql".to_owned(),
                DatabaseConnection {
                    driver: "mysql".to_owned(),
                    url: None,
                    database: Some("app".to_owned()),
                    host: Some("127.0.0.1".to_owned()),
                    port: Some(3306),
                    username: Some("phoenix".to_owned()),
                    password: Some("s3cret".to_owned()),
                },
            )]),
        };
        assert_eq!(
            resolve_database_url(&file, Some("mysql")).unwrap(),
            "mysql://phoenix:s3cret@127.0.0.1:3306/app"
        );
    }
}
