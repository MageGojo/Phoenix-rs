//! Deploy status inspection.

use std::fs;

use crate::{
    error::ReleaseError, layout::DeployLayout, manifest::ReleaseManifest,
    switch::read_current_version,
};

/// Snapshot of the deploy tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeployStatus {
    pub current_version: Option<String>,
    pub previous_version: Option<String>,
    pub releases: Vec<ReleaseInfo>,
    pub locked: bool,
}

/// Metadata for one installed release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseInfo {
    pub version: String,
    pub created_at: Option<String>,
    pub application: Option<String>,
    pub migration_count: Option<u32>,
}

/// Inspect deploy root: current version, releases, lock state.
pub fn status(deploy_root: impl Into<std::path::PathBuf>) -> Result<DeployStatus, ReleaseError> {
    let layout = DeployLayout::new(deploy_root);
    let current_version = read_current_version(&layout).ok();
    let previous_version = fs::read_to_string(layout.previous_path())
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let locked = layout.lock_path().exists();
    let releases = list_release_info(&layout)?;

    Ok(DeployStatus {
        current_version,
        previous_version,
        releases,
        locked,
    })
}

fn list_release_info(layout: &DeployLayout) -> Result<Vec<ReleaseInfo>, ReleaseError> {
    let releases_dir = layout.releases_dir();
    if !releases_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut releases = Vec::new();
    for entry in
        fs::read_dir(&releases_dir).map_err(|source| ReleaseError::io(&releases_dir, source))?
    {
        let entry = entry.map_err(|source| ReleaseError::io(&releases_dir, source))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let version = entry.file_name().to_string_lossy().into_owned();
        let manifest = ReleaseManifest::read_from(&path).ok();
        releases.push(ReleaseInfo {
            version,
            created_at: manifest.as_ref().map(|m| m.created_at.clone()),
            application: manifest.as_ref().map(|m| m.application.name.clone()),
            migration_count: manifest.as_ref().map(|m| m.migrations.count),
        });
    }

    releases.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(releases)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::Path};

    use tempfile::tempdir;

    use crate::install::{InstallOptions, InstallSource, install};
    use crate::pack::{PackOptions, StagingSources, write_staging};
    use crate::switch::read_current_version;

    use super::*;

    fn stage_sample_release(root: &Path, version: &str) -> std::path::PathBuf {
        let sources_root = root.join(format!("sources-{version}"));
        let binary = sources_root.join("bin/app");
        let manage = sources_root.join("bin/phoenix-manage");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::File::create(&binary)
            .unwrap()
            .write_all(b"app")
            .unwrap();
        fs::File::create(&manage)
            .unwrap()
            .write_all(b"#!/bin/sh\nexit 0\n")
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&manage).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&manage, perms).unwrap();
        }
        for dir in ["public", "public_ssr", "config", "migrations"] {
            fs::create_dir_all(sources_root.join(dir)).unwrap();
        }
        let staging = root.join(format!("staging-{version}"));
        write_staging(
            &PackOptions {
                version: version.into(),
                app_name: "demo".into(),
                binary_name: "app".into(),
                target_triple: "aarch64-apple-darwin".into(),
                staging_dir: staging.clone(),
                git_revision: None,
                client_manifest: None,
                ssr_manifest: None,
                contract_hash: None,
                rustc_version: None,
                profile: None,
                npm_build: None,
            },
            &StagingSources {
                binary,
                phoenix_manage: Some(manage),
                public_assets: sources_root.join("public"),
                public_ssr: sources_root.join("public_ssr"),
                config: sources_root.join("config"),
                migrations: sources_root.join("migrations"),
            },
        )
        .unwrap();
        staging
    }

    #[test]
    #[cfg(unix)]
    fn status_lists_installed_releases_and_current() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        let staging = stage_sample_release(dir.path(), "1.0.0");
        install(InstallOptions {
            deploy_root: deploy_root.clone(),
            version: "1.0.0".into(),
            source: InstallSource::Path(staging),
            skip_migrate: true,
            no_switch: false,
            restart_cmd: None,
            dry_run: false,
        })
        .unwrap();

        let snapshot = status(&deploy_root).unwrap();
        assert_eq!(snapshot.current_version.as_deref(), Some("1.0.0"));
        assert!(!snapshot.locked);
        assert_eq!(snapshot.releases.len(), 1);
        assert_eq!(snapshot.releases[0].version, "1.0.0");
        assert_eq!(snapshot.releases[0].application.as_deref(), Some("demo"));
        assert_eq!(
            read_current_version(&DeployLayout::new(&deploy_root)).unwrap(),
            "1.0.0"
        );
    }

    #[test]
    fn status_on_empty_deploy_root_is_empty() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        fs::create_dir_all(&deploy_root).unwrap();
        let snapshot = status(&deploy_root).unwrap();
        assert!(snapshot.current_version.is_none());
        assert!(snapshot.releases.is_empty());
        assert!(!snapshot.locked);
    }
}
