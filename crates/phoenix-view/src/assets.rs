use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ASSET_MANIFEST_SCHEMA: u8 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AssetManifest {
    pub schema: u8,
    pub version: String,
    pub contract_hash: String,
    #[serde(default = "default_public_path")]
    pub public_path: String,
    pub entries: HashMap<String, AssetEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AssetEntry {
    pub file: String,
    #[serde(default)]
    pub css: Vec<String>,
    #[serde(default)]
    pub imports: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct RendererManifest {
    pub schema: u8,
    pub version: String,
    pub contract_hash: String,
    pub entry: String,
}

impl RendererManifest {
    /// Load and validate the SSR renderer build manifest.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable/invalid JSON or unsafe build identity.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AssetManifestError> {
        let bytes = fs::read(path).map_err(AssetManifestError::Read)?;
        let manifest: Self = serde_json::from_slice(&bytes).map_err(AssetManifestError::Decode)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate schema, build identity, and renderer entry path.
    ///
    /// # Errors
    ///
    /// Returns an error when any field is unsafe or unsupported.
    pub fn validate(&self) -> Result<(), AssetManifestError> {
        if self.schema != ASSET_MANIFEST_SCHEMA {
            return Err(AssetManifestError::UnsupportedSchema(self.schema));
        }
        if self.version.is_empty() || self.contract_hash.is_empty() {
            return Err(AssetManifestError::MissingIdentity);
        }
        validate_asset_path(&self.entry)
    }
}

impl AssetManifest {
    /// Load and validate a production asset manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, JSON is invalid, the
    /// schema is unsupported, or any asset path can escape the asset root.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AssetManifestError> {
        let bytes = fs::read(path).map_err(AssetManifestError::Read)?;
        let manifest: Self = serde_json::from_slice(&bytes).map_err(AssetManifestError::Decode)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate the manifest without reading from disk.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported schemas, invalid public prefixes, or
    /// unsafe/duplicated asset paths.
    pub fn validate(&self) -> Result<(), AssetManifestError> {
        if self.schema != ASSET_MANIFEST_SCHEMA {
            return Err(AssetManifestError::UnsupportedSchema(self.schema));
        }
        if self.version.is_empty() || self.contract_hash.is_empty() {
            return Err(AssetManifestError::MissingIdentity);
        }
        if !self.public_path.starts_with('/') || !self.public_path.ends_with('/') {
            return Err(AssetManifestError::InvalidPublicPath(
                self.public_path.clone(),
            ));
        }
        let mut paths = HashSet::new();
        for entry in self.entries.values() {
            for path in std::iter::once(&entry.file)
                .chain(entry.css.iter())
                .chain(entry.imports.iter())
            {
                validate_asset_path(path)?;
                if !paths.insert(path) {
                    return Err(AssetManifestError::DuplicateAsset(path.clone()));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn entry(&self, name: &str) -> Option<&AssetEntry> {
        self.entries.get(name)
    }

    /// Verify that Rust and browser artifacts were generated from the same
    /// contract set.
    ///
    /// # Errors
    ///
    /// Returns a mismatch error when the expected hash differs.
    pub fn verify_contract(&self, expected: &str) -> Result<(), AssetManifestError> {
        if self.contract_hash == expected {
            Ok(())
        } else {
            Err(AssetManifestError::ContractMismatch {
                expected: expected.to_owned(),
                actual: self.contract_hash.clone(),
            })
        }
    }

    /// Produce the public URL for a manifest-owned asset.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is not present in the manifest.
    pub fn url(&self, asset: &str) -> Result<String, AssetManifestError> {
        if !self.owned_assets().contains(asset) {
            return Err(AssetManifestError::UnknownAsset(asset.to_owned()));
        }
        Ok(format!("{}{}", self.public_path, asset))
    }

    /// Resolve an incoming static URL to a file below `root`.
    ///
    /// Only exact files declared by the manifest are resolvable. This avoids
    /// using user-controlled paths as general filesystem paths.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong URL prefix or an undeclared asset.
    pub fn resolve_static(
        &self,
        root: impl AsRef<Path>,
        request_path: &str,
    ) -> Result<PathBuf, AssetManifestError> {
        let relative = request_path
            .strip_prefix(&self.public_path)
            .ok_or_else(|| AssetManifestError::UnknownAsset(request_path.to_owned()))?;
        if !self.owned_assets().contains(relative) {
            return Err(AssetManifestError::UnknownAsset(relative.to_owned()));
        }
        Ok(root.as_ref().join(relative))
    }

    fn owned_assets(&self) -> HashSet<&str> {
        self.entries
            .values()
            .flat_map(|entry| {
                std::iter::once(entry.file.as_str())
                    .chain(entry.css.iter().map(String::as_str))
                    .chain(entry.imports.iter().map(String::as_str))
            })
            .collect()
    }
}

fn default_public_path() -> String {
    "/assets/".to_owned()
}

fn validate_asset_path(path: &str) -> Result<(), AssetManifestError> {
    let candidate = Path::new(path);
    let safe = !path.is_empty()
        && !path.contains('\\')
        && !candidate.is_absolute()
        && candidate.components().all(|component| {
            matches!(component, Component::Normal(_)) && !component.as_os_str().is_empty()
        });
    if safe {
        Ok(())
    } else {
        Err(AssetManifestError::UnsafeAsset(path.to_owned()))
    }
}

#[derive(Debug, Error)]
pub enum AssetManifestError {
    #[error("failed to read the asset manifest: {0}")]
    Read(std::io::Error),
    #[error("failed to decode the asset manifest: {0}")]
    Decode(serde_json::Error),
    #[error("asset manifest schema {0} is not supported")]
    UnsupportedSchema(u8),
    #[error("asset manifest version and contract hash must be non-empty")]
    MissingIdentity,
    #[error("asset public path must begin and end with '/': {0}")]
    InvalidPublicPath(String),
    #[error("asset manifest contains an unsafe path: {0}")]
    UnsafeAsset(String),
    #[error("asset manifest declares the same file more than once: {0}")]
    DuplicateAsset(String),
    #[error("asset is not declared by the production manifest: {0}")]
    UnknownAsset(String),
    #[error("contract hash mismatch (expected {expected}, manifest has {actual})")]
    ContractMismatch { expected: String, actual: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AssetManifest {
        AssetManifest {
            schema: ASSET_MANIFEST_SCHEMA,
            version: "sha256-build".to_owned(),
            contract_hash: "fnv1a-contract".to_owned(),
            public_path: "/assets/".to_owned(),
            entries: HashMap::from([(
                "client".to_owned(),
                AssetEntry {
                    file: "phoenix-a1.js".to_owned(),
                    css: vec!["phoenix-b2.css".to_owned()],
                    imports: vec!["chunks/page-c3.js".to_owned()],
                },
            )]),
        }
    }

    #[test]
    fn resolves_only_manifest_owned_files() {
        let manifest = manifest();
        manifest.validate().unwrap();
        assert_eq!(
            manifest
                .resolve_static("public/assets", "/assets/chunks/page-c3.js")
                .unwrap(),
            PathBuf::from("public/assets/chunks/page-c3.js")
        );
        assert!(
            manifest
                .resolve_static("public/assets", "/assets/../secret")
                .is_err()
        );
        assert!(manifest.url("not-in-manifest.js").is_err());
    }

    #[test]
    fn rejects_unsafe_paths_and_contract_drift() {
        let mut invalid = manifest();
        invalid.entries.get_mut("client").unwrap().file = "../escape.js".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(AssetManifestError::UnsafeAsset(_))
        ));
        assert!(matches!(
            manifest().verify_contract("different"),
            Err(AssetManifestError::ContractMismatch { .. })
        ));
    }
}
