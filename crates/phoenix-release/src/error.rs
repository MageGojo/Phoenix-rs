//! Release pipeline error types.

use std::{io, path::PathBuf};

use thiserror::Error;

/// Errors produced by release packaging, install, and rollback.
#[derive(Debug, Error)]
pub enum ReleaseError {
    /// I/O failure.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Another release operation holds the deploy lock.
    #[error("release lock already held at {0}")]
    LockHeld(PathBuf),
    /// Checksum verification failed.
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Relative path within the release.
        path: String,
        /// Expected checksum from the manifest.
        expected: String,
        /// Computed checksum.
        actual: String,
    },
    /// Manifest parse or validation failure.
    #[error("manifest error: {0}")]
    Manifest(String),
    /// Requested release version is not present.
    #[error("release not found: {0}")]
    ReleaseNotFound(String),
    /// No previous release available for rollback.
    #[error("no previous release available for rollback")]
    NoPreviousRelease,
    /// Symlink-based deploy switching is not supported on this platform.
    #[error("atomic symlink switch is not supported on this platform")]
    UnsupportedPlatform,
    /// External command (migrate / restart) failed.
    #[error("command failed ({command}): {message}")]
    CommandFailed {
        /// Shell command that was executed.
        command: String,
        /// Failure detail (stderr or exit status).
        message: String,
    },
    /// Release layout is missing expected paths.
    #[error("invalid deploy layout: {0}")]
    InvalidLayout(String),
}

impl ReleaseError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
