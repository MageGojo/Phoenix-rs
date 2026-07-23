//! Exclusive deploy lock via `tmp/release.lock`.

use std::{
    fs::{self, File},
    path::PathBuf,
};

use crate::{error::ReleaseError, layout::DeployLayout};

/// RAII guard that removes `tmp/release.lock` on drop.
pub struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    /// Acquire the deploy lock, failing if another process holds it.
    pub fn acquire(layout: &DeployLayout) -> Result<Self, ReleaseError> {
        fs::create_dir_all(layout.tmp())
            .map_err(|source| ReleaseError::io(layout.tmp(), source))?;

        let path = layout.lock_path();
        match File::options().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Self { path }),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ReleaseError::LockHeld(path))
            }
            Err(source) => Err(ReleaseError::io(path, source)),
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
