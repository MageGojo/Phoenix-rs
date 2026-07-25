//! Roll back `current` to a previous release (no migrate down).

use std::path::PathBuf;

use crate::{
    error::ReleaseError,
    layout::DeployLayout,
    lock::LockGuard,
    status::status,
    switch::{link_shared, read_current_version, switch_current},
};

/// Options for [`rollback`].
#[derive(Clone, Debug)]
pub struct RollbackOptions {
    pub deploy_root: PathBuf,
    pub to: Option<String>,
    pub steps: usize,
    pub restart_cmd: Option<String>,
    pub skip_restart: bool,
    pub dry_run: bool,
}

/// Summary returned after a successful rollback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackReport {
    pub from: Option<String>,
    pub to: String,
    pub restarted: bool,
}

/// Switch `current` back to a previous release. Does **not** run migrate down.
#[allow(clippy::needless_pass_by_value)]
pub fn rollback(options: RollbackOptions) -> Result<RollbackReport, ReleaseError> {
    let layout = DeployLayout::new(&options.deploy_root);
    fs_ensure(&layout)?;

    let snapshot = status(&options.deploy_root)?;
    let from = snapshot.current_version.clone();

    let target = resolve_target(&options, &snapshot, from.as_deref())?;
    if from.as_deref() == Some(target.as_str()) {
        return Err(ReleaseError::InvalidLayout(format!(
            "already on release `{target}`"
        )));
    }
    let release_dir = layout.release_dir(&target);
    if !release_dir.join("manifest.toml").is_file() {
        return Err(ReleaseError::ReleaseNotFound(target));
    }

    if options.dry_run {
        return Ok(RollbackReport {
            from,
            to: target,
            restarted: false,
        });
    }

    let _lock = LockGuard::acquire(&layout)?;
    link_shared(&layout, &release_dir)?;
    if let Ok(current) = read_current_version(&layout) {
        std::fs::write(layout.previous_path(), current)
            .map_err(|source| ReleaseError::io(layout.previous_path(), source))?;
    }
    switch_current(&layout, &target)?;

    let mut restarted = false;
    if !options.skip_restart {
        if let Some(command) = &options.restart_cmd {
            run_shell(command)?;
            restarted = true;
        } else if layout.root().join("deploy/restart.sh").is_file() {
            run_shell(&format!(
                "sh {}",
                layout.root().join("deploy/restart.sh").display()
            ))?;
            restarted = true;
        }
    }

    Ok(RollbackReport {
        from,
        to: target,
        restarted,
    })
}

fn resolve_target(
    options: &RollbackOptions,
    snapshot: &crate::status::DeployStatus,
    from: Option<&str>,
) -> Result<String, ReleaseError> {
    if let Some(version) = &options.to {
        return Ok(version.clone());
    }
    if options.steps == 0 {
        return Err(ReleaseError::InvalidLayout(
            "rollback steps must be >= 1".into(),
        ));
    }
    if let Some(previous) = snapshot
        .previous_version
        .as_ref()
        .filter(|version| from != Some(version.as_str()))
    {
        return Ok(previous.clone());
    }
    let mut versions: Vec<String> = snapshot
        .releases
        .iter()
        .map(|info| info.version.clone())
        .collect();
    if let Some(current) = from {
        versions.retain(|version| version != current);
    }
    versions.sort();
    versions
        .into_iter()
        .rev()
        .nth(options.steps.saturating_sub(1))
        .ok_or(ReleaseError::NoPreviousRelease)
}

fn fs_ensure(layout: &DeployLayout) -> Result<(), ReleaseError> {
    for dir in [layout.releases_dir(), layout.shared(), layout.tmp()] {
        std::fs::create_dir_all(&dir).map_err(|source| ReleaseError::io(&dir, source))?;
    }
    Ok(())
}

fn run_shell(command: &str) -> Result<(), ReleaseError> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|err| ReleaseError::CommandFailed {
            command: command.into(),
            message: err.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(ReleaseError::CommandFailed {
            command: command.into(),
            message: format!("status={:?}; stderr={stderr}", output.status),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::Path};

    use tempfile::tempdir;

    use crate::install::{InstallOptions, InstallSource, install};
    use crate::pack::{PackOptions, StagingSources, write_staging};
    use crate::switch::read_current_version;

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

    fn install_version(deploy_root: &Path, sources_root: &Path, version: &str) {
        let staging = stage_sample_release(sources_root, version);
        install(InstallOptions {
            deploy_root: deploy_root.to_path_buf(),
            version: version.into(),
            source: InstallSource::Path(staging),
            skip_migrate: true,
            no_switch: false,
            restart_cmd: None,
            dry_run: false,
        })
        .unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn rollback_dry_run_targets_previous_without_switching() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        install_version(&deploy_root, dir.path(), "1.0.0");
        install_version(&deploy_root, dir.path(), "1.1.0");
        assert_eq!(
            read_current_version(&DeployLayout::new(&deploy_root)).unwrap(),
            "1.1.0"
        );

        let report = rollback(RollbackOptions {
            deploy_root: deploy_root.clone(),
            to: Some("1.0.0".into()),
            steps: 1,
            restart_cmd: None,
            skip_restart: true,
            dry_run: true,
        })
        .unwrap();
        assert_eq!(report.from.as_deref(), Some("1.1.0"));
        assert_eq!(report.to, "1.0.0");
        assert!(!report.restarted);
        assert_eq!(
            read_current_version(&DeployLayout::new(&deploy_root)).unwrap(),
            "1.1.0"
        );
    }

    #[test]
    #[cfg(unix)]
    fn rollback_switches_to_explicit_previous_release() {
        let dir = tempdir().unwrap();
        let deploy_root = dir.path().join("deploy");
        install_version(&deploy_root, dir.path(), "1.0.0");
        install_version(&deploy_root, dir.path(), "1.1.0");

        let report = rollback(RollbackOptions {
            deploy_root: deploy_root.clone(),
            to: Some("1.0.0".into()),
            steps: 1,
            restart_cmd: None,
            skip_restart: true,
            dry_run: false,
        })
        .unwrap();
        assert_eq!(report.from.as_deref(), Some("1.1.0"));
        assert_eq!(report.to, "1.0.0");
        assert_eq!(
            read_current_version(&DeployLayout::new(&deploy_root)).unwrap(),
            "1.0.0"
        );
    }
}
