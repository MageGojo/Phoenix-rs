//! Atomic `current` symlink switching and shared resource links.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{error::ReleaseError, layout::DeployLayout};

/// Read the active release version from the `current` symlink.
pub fn read_current_version(layout: &DeployLayout) -> Result<String, ReleaseError> {
    let current = layout.current();
    let target = fs::read_link(&current).map_err(|source| ReleaseError::io(&current, source))?;
    version_from_release_path(&target, layout)
}

/// Atomically point `current` at `releases/<version>`.
pub fn switch_current(layout: &DeployLayout, version: &str) -> Result<(), ReleaseError> {
    let release_dir = layout.release_dir(version);
    if !release_dir.is_dir() {
        return Err(ReleaseError::ReleaseNotFound(version.into()));
    }

    #[cfg(unix)]
    {
        switch_current_unix(layout, &release_dir)
    }
    #[cfg(not(unix))]
    {
        let _ = (layout, version);
        Err(ReleaseError::UnsupportedPlatform)
    }
}

/// Symlink release-local `.env` and `storage` to shared persistent paths.
pub fn link_shared(layout: &DeployLayout, release_dir: &Path) -> Result<(), ReleaseError> {
    fs::create_dir_all(layout.shared())
        .map_err(|source| ReleaseError::io(layout.shared(), source))?;
    fs::create_dir_all(layout.shared_storage())
        .map_err(|source| ReleaseError::io(layout.shared_storage(), source))?;

    let env_link = release_dir.join(".env");
    let storage_link = release_dir.join("storage");

    replace_symlink(&env_link, &layout.shared_env())?;
    replace_symlink(&storage_link, &layout.shared_storage())?;
    Ok(())
}

#[cfg(unix)]
fn switch_current_unix(layout: &DeployLayout, release_dir: &Path) -> Result<(), ReleaseError> {
    use std::os::unix::fs::symlink;

    fs::create_dir_all(layout.tmp()).map_err(|source| ReleaseError::io(layout.tmp(), source))?;

    let new_current = layout.new_current_path();
    if new_current.exists() {
        fs::remove_file(&new_current).map_err(|source| ReleaseError::io(&new_current, source))?;
    }

    symlink(release_dir, &new_current).map_err(|source| ReleaseError::io(&new_current, source))?;
    fs::rename(&new_current, layout.current())
        .map_err(|source| ReleaseError::io(layout.current(), source))?;
    Ok(())
}

fn replace_symlink(link: &Path, target: &Path) -> Result<(), ReleaseError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        if link.exists() || link.is_symlink() {
            fs::remove_file(link).map_err(|source| ReleaseError::io(link, source))?;
        }
        symlink(target, link).map_err(|source| ReleaseError::io(link, source))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        if !target.exists() {
            if target.extension().is_some() {
                fs::File::create(target).map_err(|source| ReleaseError::io(target, source))?;
            } else {
                fs::create_dir_all(target).map_err(|source| ReleaseError::io(target, source))?;
            }
        }
        if link.exists() {
            fs::remove_file(link).map_err(|source| ReleaseError::io(link, source))?;
        }
        std::os::windows::fs::symlink_dir(target, link)
            .or_else(|_| std::os::windows::fs::symlink_file(target, link))
            .map_err(|source| ReleaseError::io(link, source))
    }
}

fn version_from_release_path(path: &Path, layout: &DeployLayout) -> Result<String, ReleaseError> {
    let releases = layout.releases_dir();
    let normalized = normalize_path(path);

    if let Ok(relative) = normalized.strip_prefix(&releases) {
        let version = relative
            .components()
            .next()
            .ok_or_else(|| ReleaseError::InvalidLayout("current symlink has no version".into()))?
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        return Ok(version);
    }

    normalized
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| ReleaseError::InvalidLayout("unable to parse current symlink".into()))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}
