//! SHA-256 checksum helpers.

use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::{error::ReleaseError, manifest::ReleaseManifest};

/// Compute the SHA-256 digest of `path`, formatted as `sha256:<hex>`.
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, ReleaseError> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| ReleaseError::io(path, source))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| ReleaseError::io(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!(
        "sha256:{}",
        hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut out, byte| {
                use std::fmt::Write;
                let _ = write!(out, "{byte:02x}");
                out
            })
    ))
}

/// Verify every checksum entry in `manifest` against files under `release_dir`.
pub fn verify_checksums(
    release_dir: impl AsRef<Path>,
    manifest: &ReleaseManifest,
) -> Result<(), ReleaseError> {
    let release_dir = release_dir.as_ref();

    for (relative, expected) in &manifest.checksums {
        if relative == "tarball" || relative == "manifest.toml" {
            continue;
        }

        let file_path = release_dir.join(relative);
        if !file_path.is_file() {
            return Err(ReleaseError::ChecksumMismatch {
                path: relative.clone(),
                expected: expected.clone(),
                actual: "missing file".into(),
            });
        }

        let actual = sha256_file(&file_path)?;
        if actual != *expected {
            return Err(ReleaseError::ChecksumMismatch {
                path: relative.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::manifest::{
        ApplicationMeta, AssetsMeta, BuildMeta, MANIFEST_SCHEMA_VERSION, MigrationsMeta,
        ReleaseManifest,
    };

    #[test]
    fn sha256_file_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"hello")
            .unwrap();

        let digest = sha256_file(&path).unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(
            digest,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn checksum_verify_fails_on_tampered_file() {
        let dir = tempdir().unwrap();
        let release_dir = dir.path().join("release");
        std::fs::create_dir_all(release_dir.join("bin")).unwrap();
        let binary = release_dir.join("bin/app");
        std::fs::File::create(&binary)
            .unwrap()
            .write_all(b"original")
            .unwrap();

        let checksum = sha256_file(&binary).unwrap();
        let manifest = ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            version: "1.0.0".into(),
            created_at: "2026-07-23T12:00:00Z".into(),
            git_revision: None,
            application: ApplicationMeta {
                name: "demo".into(),
                binary: "app".into(),
                target_triple: "aarch64-apple-darwin".into(),
            },
            assets: AssetsMeta::default(),
            migrations: MigrationsMeta {
                included: false,
                count: 0,
            },
            checksums: std::collections::BTreeMap::from([("bin/app".into(), checksum)]),
            build: BuildMeta::default(),
        };

        verify_checksums(&release_dir, &manifest).unwrap();

        std::fs::File::create(&binary)
            .unwrap()
            .write_all(b"tampered")
            .unwrap();

        assert!(matches!(
            verify_checksums(&release_dir, &manifest),
            Err(ReleaseError::ChecksumMismatch { .. })
        ));
    }
}
