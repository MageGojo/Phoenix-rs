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
