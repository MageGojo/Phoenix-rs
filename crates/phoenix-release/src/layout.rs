//! Deploy directory layout helpers.

use std::path::{Path, PathBuf};

/// Root of a Phoenix-rs deploy tree (`releases/`, `current`, `shared/`, `tmp/`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeployLayout {
    root: PathBuf,
}

impl DeployLayout {
    /// Construct a layout rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Deploy root path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `$DEPLOY_ROOT/releases/`
    #[must_use]
    pub fn releases_dir(&self) -> PathBuf {
        self.root.join("releases")
    }

    /// `$DEPLOY_ROOT/releases/<version>/`
    #[must_use]
    pub fn release_dir(&self, version: &str) -> PathBuf {
        self.releases_dir().join(version)
    }

    /// `$DEPLOY_ROOT/current` (symlink to active release).
    #[must_use]
    pub fn current(&self) -> PathBuf {
        self.root.join("current")
    }

    /// `$DEPLOY_ROOT/shared/`
    #[must_use]
    pub fn shared(&self) -> PathBuf {
        self.root.join("shared")
    }

    /// `$DEPLOY_ROOT/tmp/`
    #[must_use]
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// `$DEPLOY_ROOT/tmp/release.lock`
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.tmp().join("release.lock")
    }

    /// `$DEPLOY_ROOT/tmp/previous` (text file with prior version id).
    #[must_use]
    pub fn previous_path(&self) -> PathBuf {
        self.tmp().join("previous")
    }

    /// `$DEPLOY_ROOT/tmp/new-current` (staging symlink for atomic switch).
    #[must_use]
    pub fn new_current_path(&self) -> PathBuf {
        self.tmp().join("new-current")
    }

    /// `$DEPLOY_ROOT/shared/.env`
    #[must_use]
    pub fn shared_env(&self) -> PathBuf {
        self.shared().join(".env")
    }

    /// `$DEPLOY_ROOT/shared/storage/`
    #[must_use]
    pub fn shared_storage(&self) -> PathBuf {
        self.shared().join("storage")
    }
}
