//! Release manifest schema (TOML, `schema_version = 1`).

use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::ReleaseError;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.toml";

/// Full release manifest written to `releases/<ver>/manifest.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_revision: Option<String>,
    pub application: ApplicationMeta,
    #[serde(default)]
    pub assets: AssetsMeta,
    #[serde(default)]
    pub migrations: MigrationsMeta,
    #[serde(default)]
    pub checksums: BTreeMap<String, String>,
    #[serde(default)]
    pub build: BuildMeta,
}

/// Application binary metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApplicationMeta {
    pub name: String,
    pub binary: String,
    pub target_triple: String,
}

/// Frontend / contract asset metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetsMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssr_manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_hash: Option<String>,
}

/// Database migration bundle metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationsMeta {
    pub included: bool,
    pub count: u32,
}

/// Build provenance metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustc_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_build: Option<String>,
}

impl ReleaseManifest {
    /// Serialize to TOML text.
    pub fn to_toml(&self) -> Result<String, ReleaseError> {
        toml::to_string_pretty(self).map_err(|err| ReleaseError::Manifest(err.to_string()))
    }

    /// Parse from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, ReleaseError> {
        let manifest: Self =
            toml::from_str(text).map_err(|err| ReleaseError::Manifest(err.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read `manifest.toml` from a release directory.
    pub fn read_from(release_dir: impl AsRef<Path>) -> Result<Self, ReleaseError> {
        let path = release_dir.as_ref().join(MANIFEST_FILE);
        let text = fs::read_to_string(&path).map_err(|source| ReleaseError::io(&path, source))?;
        Self::from_toml(&text)
    }

    /// Write `manifest.toml` into a release directory.
    pub fn write_to(&self, release_dir: impl AsRef<Path>) -> Result<(), ReleaseError> {
        let release_dir = release_dir.as_ref();
        fs::create_dir_all(release_dir).map_err(|source| ReleaseError::io(release_dir, source))?;
        let path = release_dir.join(MANIFEST_FILE);
        fs::write(&path, self.to_toml()?).map_err(|source| ReleaseError::io(&path, source))
    }

    fn validate(&self) -> Result<(), ReleaseError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ReleaseError::Manifest(format!(
                "unsupported schema_version {} (expected {MANIFEST_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        if self.version.trim().is_empty() {
            return Err(ReleaseError::Manifest("version must not be empty".into()));
        }
        if self.application.binary.trim().is_empty() {
            return Err(ReleaseError::Manifest(
                "application.binary must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            version: "1.0.0".into(),
            created_at: "2026-07-23T12:00:00Z".into(),
            git_revision: Some("abc123".into()),
            application: ApplicationMeta {
                name: "blog".into(),
                binary: "blog".into(),
                target_triple: "aarch64-apple-darwin".into(),
            },
            assets: AssetsMeta {
                client_manifest: Some("public/manifest.json".into()),
                ssr_manifest: None,
                contract_hash: Some("sha256:deadbeef".into()),
            },
            migrations: MigrationsMeta {
                included: true,
                count: 2,
            },
            checksums: BTreeMap::from([("bin/blog".into(), "sha256:0123456789abcdef".into())]),
            build: BuildMeta {
                rustc_version: Some("1.95.0".into()),
                profile: Some("release".into()),
                npm_build: Some("vite build".into()),
            },
        }
    }

    #[test]
    fn manifest_roundtrip() {
        let manifest = sample_manifest();
        let text = manifest.to_toml().unwrap();
        let parsed = ReleaseManifest::from_toml(&text).unwrap();
        assert_eq!(manifest, parsed);
    }
}
