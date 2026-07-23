//! Install a release into the deploy layout.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    checksum::verify_checksums,
    error::ReleaseError,
    layout::DeployLayout,
    lock::LockGuard,
    manifest::ReleaseManifest,
    pack::extract_tarball,
    switch::{link_shared, read_current_version, switch_current},
};

/// Where the release payload comes from.
#[derive(Clone, Debug)]
pub enum InstallSource {
    /// A `.tar.gz` produced by [`crate::pack::create_tarball`].
    Tarball(PathBuf),
    /// An already-staged release directory.
    Path(PathBuf),
}

/// Options for [`install`].
#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub deploy_root: PathBuf,
    pub version: String,
    pub source: InstallSource,
    pub skip_migrate: bool,
    pub no_switch: bool,
    pub restart_cmd: Option<String>,
    pub dry_run: bool,
}

/// Summary returned after a successful install.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallReport {
    pub version: String,
    pub release_dir: PathBuf,
    pub switched: bool,
    pub migrated: bool,
    pub restarted: bool,
    pub previous_version: Option<String>,
}

/// Install a release: lock, extract, verify, link shared, migrate, switch, restart.
pub fn install(options: InstallOptions) -> Result<InstallReport, ReleaseError> {
    let layout = DeployLayout::new(&options.deploy_root);
    ensure_layout(&layout)?;

    let release_dir = layout.release_dir(&options.version);
    let previous_version = read_current_version(&layout).ok();

    if options.dry_run {
        return Ok(InstallReport {
            version: options.version.clone(),
            release_dir,
            switched: !options.no_switch,
            migrated: !options.skip_migrate,
            restarted: options.restart_cmd.is_some(),
            previous_version,
        });
    }

    let _lock = LockGuard::acquire(&layout)?;

    if release_dir.exists() {
        fs::remove_dir_all(&release_dir)
            .map_err(|source| ReleaseError::io(&release_dir, source))?;
    }
    fs::create_dir_all(&release_dir).map_err(|source| ReleaseError::io(&release_dir, source))?;

    match &options.source {
        InstallSource::Tarball(path) => extract_tarball(path, &release_dir)?,
        InstallSource::Path(path) => copy_release_tree(path, &release_dir)?,
    }

    let manifest = ReleaseManifest::read_from(&release_dir)?;
    if manifest.version != options.version {
        return Err(ReleaseError::Manifest(format!(
            "manifest version {} does not match requested {}",
            manifest.version, options.version
        )));
    }

    verify_checksums(&release_dir, &manifest)?;
    link_shared(&layout, &release_dir)?;

    let migrated = if options.skip_migrate {
        false
    } else if manifest.migrations.included {
        run_migrate(&release_dir)?;
        true
    } else {
        false
    };

    let mut switched = false;
    if !options.no_switch {
        if let Ok(current) = read_current_version(&layout) {
            write_previous(&layout, &current)?;
        }
        switch_current(&layout, &options.version)?;
        switched = true;
    }

    let restarted = if let Some(cmd) = &options.restart_cmd {
        run_shell(cmd)?;
        true
    } else {
        false
    };

    Ok(InstallReport {
        version: options.version,
        release_dir,
        switched,
        migrated,
        restarted,
        previous_version,
    })
}

fn ensure_layout(layout: &DeployLayout) -> Result<(), ReleaseError> {
    for dir in [
        layout.releases_dir(),
        layout.shared(),
        layout.tmp(),
        layout.shared_storage(),
    ] {
        fs::create_dir_all(&dir).map_err(|source| ReleaseError::io(&dir, source))?;
    }
    Ok(())
}

fn copy_release_tree(from: &Path, to: &Path) -> Result<(), ReleaseError> {
    if !from.is_dir() {
        return Err(ReleaseError::InvalidLayout(format!(
            "source is not a directory: {}",
            from.display()
        )));
    }
    copy_tree_recursive(from, to)
}

fn copy_tree_recursive(from: &Path, to: &Path) -> Result<(), ReleaseError> {
    fs::create_dir_all(to).map_err(|source| ReleaseError::io(to, source))?;
    for entry in fs::read_dir(from).map_err(|source| ReleaseError::io(from, source))? {
        let entry = entry.map_err(|source| ReleaseError::io(from, source))?;
        let src = entry.path();
        let dest = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree_recursive(&src, &dest)?;
        } else {
            fs::copy(&src, &dest).map_err(|source| ReleaseError::io(&dest, source))?;
        }
    }
    Ok(())
}

fn write_previous(layout: &DeployLayout, version: &str) -> Result<(), ReleaseError> {
    fs::create_dir_all(layout.tmp()).map_err(|source| ReleaseError::io(layout.tmp(), source))?;
    fs::write(layout.previous_path(), version)
        .map_err(|source| ReleaseError::io(layout.previous_path(), source))
}

fn run_migrate(release_dir: &Path) -> Result<(), ReleaseError> {
    let manage = release_dir.join("bin/phoenix-manage");
    if !manage.is_file() {
        return Err(ReleaseError::InvalidLayout(format!(
            "missing migrate binary: {}",
            manage.display()
        )));
    }

    let output = Command::new(&manage)
        .arg("migrate")
        .current_dir(release_dir)
        .output()
        .map_err(|err| ReleaseError::CommandFailed {
            command: format!("{} migrate", manage.display()),
            message: err.to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(ReleaseError::CommandFailed {
            command: format!("{} migrate", manage.display()),
            message: format!(
                "status={:?}; stdout={stdout}; stderr={stderr}",
                output.status
            ),
        });
    }
    Ok(())
}

fn run_shell(command: &str) -> Result<(), ReleaseError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|err| ReleaseError::CommandFailed {
            command: command.into(),
            message: err.to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ReleaseError::CommandFailed {
            command: command.into(),
            message: format!("status={:?}; stderr={stderr}", output.status),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use crate::pack::{PackOptions, StagingSources, write_staging};

    use super::*;

    fn stage_sample_release(root: &Path, version: &str) -> PathBuf {
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
                phoenix_manage: manage,
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
    fn install_dry_run_does_not_switch() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        let staging = stage_sample_release(dir.path(), "1.0.0");

        let report = install(InstallOptions {
            deploy_root: deploy_root.clone(),
            version: "1.0.0".into(),
            source: InstallSource::Path(staging),
            skip_migrate: true,
            no_switch: false,
            restart_cmd: None,
            dry_run: true,
        })
        .unwrap();

        assert!(report.switched);
        assert!(!deploy_root.join("current").exists());
    }

    #[test]
    #[cfg(unix)]
    fn install_switches_current_on_unix() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        let staging = stage_sample_release(dir.path(), "1.0.0");

        let report = install(InstallOptions {
            deploy_root: deploy_root.clone(),
            version: "1.0.0".into(),
            source: InstallSource::Path(staging),
            skip_migrate: true,
            no_switch: false,
            restart_cmd: None,
            dry_run: false,
        })
        .unwrap();

        assert!(report.switched);
        let layout = DeployLayout::new(&deploy_root);
        assert_eq!(read_current_version(&layout).unwrap(), "1.0.0");
    }
}
