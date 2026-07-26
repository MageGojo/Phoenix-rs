//! Per-Feature TOML configuration: `config/<name>.toml` + `.env` injection.
//!
//! Structure lives in committed TOML; secrets stay in `.env` and are injected
//! into string values through `${VAR_NAME}` placeholders before deserialization.

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use serde::de::DeserializeOwned;

use crate::ConfigError;

/// Load one Feature's configuration from `config/<name>.toml`.
///
/// Designed for scaffolded Feature assembly code
/// (e.g. `load_feature_config::<PayFileConfig>("pay")`):
///
/// - A missing file (or missing `config/` directory) returns `T::default()`,
///   so Features always boot with zero configuration.
/// - String values may reference environment variables as `${VAR_NAME}`;
///   values are substituted from the process environment with `.env` as a
///   fallback before parsing, so secrets never live in the committed TOML.
///   Unknown variables become empty strings.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name is not a plain identifier, the file
/// cannot be read, or the (substituted) TOML does not deserialize into `T`.
pub fn load_feature_config<T>(name: &str) -> Result<T, ConfigError>
where
    T: DeserializeOwned + Default,
{
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return Err(ConfigError::InvalidOwned {
            key: "feature".to_owned(),
            reason: format!("feature config name `{name}` must be a plain file name"),
        });
    }
    let path = feature_config_path(name)?;
    if !path.is_file() {
        return Ok(T::default());
    }
    let source = fs::read_to_string(&path).map_err(|error| ConfigError::InvalidOwned {
        key: "feature".to_owned(),
        reason: format!("failed to read {}: {error}", path.display()),
    })?;
    let substituted = substitute_env(&source, &dotenv_values());
    toml::from_str(&substituted).map_err(|error| ConfigError::InvalidOwned {
        key: "feature".to_owned(),
        reason: format!("invalid TOML in {}: {error}", path.display()),
    })
}

fn feature_config_path(name: &str) -> Result<PathBuf, ConfigError> {
    let current = env::current_dir().map_err(|error| ConfigError::InvalidOwned {
        key: "feature".to_owned(),
        reason: format!("cannot read current directory: {error}"),
    })?;
    Ok(current.join("config").join(format!("{name}.toml")))
}

/// `.env` values as substitution fallback; process environment always wins.
fn dotenv_values() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    if let Ok(iter) = dotenvy::dotenv_iter() {
        for (key, value) in iter.flatten() {
            values.insert(key, value);
        }
    }
    values
}

/// Replace `${VAR_NAME}` placeholders (ASCII letters, digits, `_`).
fn substitute_env(source: &str, fallback: &BTreeMap<String, String>) -> String {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("${") {
        let (before, after) = rest.split_at(start);
        output.push_str(before);
        let body = &after[2..];
        let Some(end) = body.find('}') else {
            output.push_str(after);
            return output;
        };
        let variable = &body[..end];
        let valid = !variable.is_empty()
            && variable
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if valid {
            match env::var(variable) {
                Ok(value) => output.push_str(&value),
                Err(_) => output.push_str(fallback.get(variable).map_or("", String::as_str)),
            }
        } else {
            // Not a variable reference; keep the literal text.
            output.push_str(&after[..start_len(end)]);
        }
        rest = &body[end + 1..];
    }
    output.push_str(rest);
    output
}

/// Length of the literal `${...}` span given the `}` offset inside the body.
const fn start_len(end: usize) -> usize {
    // "${" + body up to and including "}"
    2 + end + 1
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Default, Deserialize, Eq, PartialEq)]
    struct DemoConfig {
        #[serde(default)]
        token: String,
        #[serde(default)]
        count: u32,
    }

    #[test]
    fn substitution_reads_environment_and_keeps_literals() {
        let path = env::var("PATH").expect("PATH is set in test environments");
        let fallback = BTreeMap::from([("ONLY_DOTENV".to_owned(), "dot".to_owned())]);
        let source = "a = \"${PATH}\"\nb = \"${ONLY_DOTENV}\"\nc = \"${MISSING_VAR_XYZ}\"\nd = \"${not-valid}\"\n";
        let substituted = substitute_env(source, &fallback);
        assert!(substituted.contains(&format!("a = \"{path}\"")));
        assert!(substituted.contains("b = \"dot\""));
        assert!(substituted.contains("c = \"\""));
        assert!(substituted.contains("d = \"${not-valid}\""));
    }

    #[test]
    fn missing_file_returns_default_and_bad_names_fail() {
        let loaded: DemoConfig =
            load_feature_config("definitely-missing-feature-config").expect("default");
        assert_eq!(loaded, DemoConfig::default());
        assert!(load_feature_config::<DemoConfig>("../escape").is_err());
        assert!(load_feature_config::<DemoConfig>("").is_err());
    }
}
