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
