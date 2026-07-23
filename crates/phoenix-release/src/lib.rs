//! Phoenix-rs release packaging, install, rollback, and deploy layout.

#![doc = include_str!("../README.md")]
#![allow(clippy::missing_errors_doc)]

mod error;
mod lock;
mod switch;

pub mod checksum;
pub mod install;
pub mod layout;
pub mod manifest;
pub mod pack;
pub mod rollback;
pub mod status;

pub use checksum::{sha256_file, verify_checksums};
pub use error::ReleaseError;
pub use install::{InstallOptions, InstallReport, InstallSource, install};
pub use layout::DeployLayout;
pub use manifest::{ApplicationMeta, AssetsMeta, BuildMeta, MigrationsMeta, ReleaseManifest};
pub use pack::{PackOptions, StagingSources, create_tarball, extract_tarball, write_staging};
pub use rollback::{RollbackOptions, RollbackReport, rollback};
pub use status::{DeployStatus, ReleaseInfo, status};
