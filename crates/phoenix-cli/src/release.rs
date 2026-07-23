#![allow(
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::map_unwrap_or
)]

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use phoenix_release::{
    DeployStatus, InstallOptions, InstallSource, PackOptions, ReleaseError, RollbackOptions,
    StagingSources, create_tarball, extract_tarball, install, rollback, status, write_staging,
};

use crate::ProjectGenerator;

pub fn release_build(args: Vec<String>) -> Result<(), String> {
    let options = parse_build_args(args)?;
    let generator = ProjectGenerator::discover(env::current_dir().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let root = generator.root();

    let package = read_cargo_package(root)?;
    let version = options
        .version
        .clone()
        .unwrap_or_else(|| package.version.clone());
    let binary_name = options
        .bin
        .clone()
        .unwrap_or_else(|| package.default_run.unwrap_or(package.name.clone()));
    let target_triple = options.target.clone().unwrap_or_else(host_triple);
    let output = options
        .output
        .clone()
        .unwrap_or_else(|| root.join("dist/releases").join(&version));
    let staging_dir = output.join("staging");

    if !options.skip_types && root.join("node_modules").is_dir() {
        run_command("npm", &["run", "types"], root, "npm run types")?;
    }
    if !options.skip_npm && root.join("node_modules").is_dir() {
        run_command("npm", &["run", "build"], root, "npm run build")?;
    }

    let mut cargo_args = vec!["build".to_owned(), "--release".to_owned()];
    if let Some(target) = &options.target {
        cargo_args.push("--target".to_owned());
        cargo_args.push(target.clone());
    }
    cargo_args.push("--bin".to_owned());
    cargo_args.push(binary_name.clone());
    cargo_args.push("--bin".to_owned());
    cargo_args.push("phoenix-manage".to_owned());

    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    run_command_strings(
        &cargo.to_string_lossy(),
        &cargo_args,
        root,
        "cargo build --release",
    )?;

    let release_dir = release_profile_dir(root, options.target.as_deref());
    let binary_path = release_dir.join(&binary_name);
    if !binary_path.is_file() {
        return Err(format!(
            "release binary missing at {}",
            binary_path.display()
        ));
    }
    let manage_path = release_dir.join("phoenix-manage");
    if !manage_path.is_file() {
        return Err(format!(
            "phoenix-manage binary missing at {}",
            manage_path.display()
        ));
    }

    let public_assets = ensure_dir(root.join("public/assets"))?;
    let public_ssr = ensure_dir(root.join("public/ssr"))?;
    let config = ensure_dir(root.join("config"))?;
    let migrations = ensure_dir(root.join("database/migrations"))?;

    let client_manifest = root.join("public/assets/phoenix-manifest.json");
    let ssr_manifest = root.join("public/ssr/phoenix-manifest.json");
    let contract_hash = read_json_str_field(&client_manifest, "contract_hash")
        .or_else(|| read_json_str_field(&ssr_manifest, "contract_hash"));

    let manifest = write_staging(
        &PackOptions {
            version: version.clone(),
            app_name: package.name.clone(),
            binary_name: binary_name.clone(),
            target_triple: target_triple.clone(),
            staging_dir: staging_dir.clone(),
            git_revision: git_revision(root),
            client_manifest: read_json_str_field(&client_manifest, "version")
                .map(|_| "public/assets/phoenix-manifest.json".to_owned()),
            ssr_manifest: read_json_str_field(&ssr_manifest, "version")
                .map(|_| "public/ssr/phoenix-manifest.json".to_owned()),
            contract_hash,
            rustc_version: rustc_version(),
            profile: Some("release".to_owned()),
            npm_build: if options.skip_npm {
                None
            } else {
                Some("npm run build".to_owned())
            },
        },
        &StagingSources {
            binary: binary_path,
            phoenix_manage: manage_path,
            public_assets,
            public_ssr,
            config,
            migrations,
        },
    )
    .map_err(release_err)?;

    println!("STAGING {}", staging_dir.display());
    println!("MANIFEST {}", staging_dir.join("manifest.toml").display());
    println!("VERSION {}", manifest.version);

    if options.tarball {
        fs::create_dir_all(&output).map_err(|e| e.to_string())?;
        let tarball = output.join(format!("{}-{}.tar.gz", package.name, version));
        let digest = create_tarball(&staging_dir, &tarball).map_err(release_err)?;
        println!("TARBALL {}", tarball.display());
        println!("SHA256 {digest}");
    }

    Ok(())
}

pub fn release_install(args: Vec<String>) -> Result<(), String> {
    let options = parse_install_args(args)?;
    let deploy_root = resolve_deploy_root(options.deploy_root)?;
    let version = resolve_install_version(options.version.as_deref(), &options.source)?;
    let source = match options.source {
        InstallSourceKind::Tarball(path) => InstallSource::Tarball(path),
        InstallSourceKind::Path(path) => InstallSource::Path(path),
    };

    let report = install(InstallOptions {
        deploy_root: deploy_root.clone(),
        version: version.clone(),
        source,
        skip_migrate: options.skip_migrate,
        no_switch: options.no_switch,
        restart_cmd: options.restart_cmd.clone(),
        dry_run: options.dry_run,
    })
    .map_err(release_err)?;

    if options.dry_run {
        println!("DRY RUN install {version} into {}", deploy_root.display());
    }
    println!("INSTALLED {}", report.version);
    println!("RELEASE_DIR {}", report.release_dir.display());
    if let Some(previous) = report.previous_version {
        println!("PREVIOUS {previous}");
    }
    println!(
        "SWITCHED={} MIGRATED={} RESTARTED={}",
        report.switched, report.migrated, report.restarted
    );
    Ok(())
}

pub fn release_rollback(args: Vec<String>) -> Result<(), String> {
    let options = parse_rollback_args(args)?;
    let deploy_root = resolve_deploy_root(options.deploy_root)?;

    let report = rollback(RollbackOptions {
        deploy_root: deploy_root.clone(),
        to: options.to,
        steps: options.steps,
        restart_cmd: options.restart_cmd,
        skip_restart: options.skip_restart,
        dry_run: options.dry_run,
    })
    .map_err(release_err)?;

    if options.dry_run {
        println!("DRY RUN rollback on {}", deploy_root.display());
    }
    if let Some(from) = report.from {
        println!("FROM {from}");
    }
    println!("TO {}", report.to);
    println!("RESTARTED={}", report.restarted);
    Ok(())
}

pub fn release_status(args: Vec<String>) -> Result<(), String> {
    let options = parse_status_args(args)?;
    let deploy_root = resolve_deploy_root(options.deploy_root)?;
    let snapshot = status(&deploy_root).map_err(release_err)?;

    if options.json {
        print_status_json(&snapshot);
    } else {
        print_status_human(&deploy_root, &snapshot);
    }
    Ok(())
}

struct CargoPackage {
    name: String,
    version: String,
    default_run: Option<String>,
}

struct BuildOptions {
    version: Option<String>,
    output: Option<PathBuf>,
    tarball: bool,
    bin: Option<String>,
    skip_npm: bool,
    skip_types: bool,
    target: Option<String>,
}

enum InstallSourceKind {
    Tarball(PathBuf),
    Path(PathBuf),
}

struct InstallOptionsParsed {
    deploy_root: Option<PathBuf>,
    source: InstallSourceKind,
    version: Option<String>,
    skip_migrate: bool,
    no_switch: bool,
    restart_cmd: Option<String>,
    dry_run: bool,
}

struct RollbackOptionsParsed {
    deploy_root: Option<PathBuf>,
    to: Option<String>,
    steps: usize,
    restart_cmd: Option<String>,
    skip_restart: bool,
    dry_run: bool,
}

struct StatusOptions {
    deploy_root: Option<PathBuf>,
    json: bool,
}

fn parse_build_args(args: Vec<String>) -> Result<BuildOptions, String> {
    let mut options = BuildOptions {
        version: None,
        output: None,
        tarball: false,
        bin: None,
        skip_npm: false,
        skip_types: false,
        target: None,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--version" => {
                index += 1;
                options.version = Some(next_value(&args, index, "--version")?);
            }
            "--output" => {
                index += 1;
                options.output = Some(PathBuf::from(next_value(&args, index, "--output")?));
            }
            "--tarball" => options.tarball = true,
            "--bin" => {
                index += 1;
                options.bin = Some(next_value(&args, index, "--bin")?);
            }
            "--skip-npm" => options.skip_npm = true,
            "--skip-types" => options.skip_types = true,
            "--target" => {
                index += 1;
                options.target = Some(next_value(&args, index, "--target")?);
            }
            flag if flag.starts_with("--version=") => {
                options.version = Some(flag["--version=".len()..].to_owned());
            }
            flag if flag.starts_with("--output=") => {
                options.output = Some(PathBuf::from(flag["--output=".len()..].to_owned()));
            }
            flag if flag.starts_with("--bin=") => {
                options.bin = Some(flag["--bin=".len()..].to_owned());
            }
            flag if flag.starts_with("--target=") => {
                options.target = Some(flag["--target=".len()..].to_owned());
            }
            flag => return Err(format!("unknown release option `{flag}`")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_install_args(args: Vec<String>) -> Result<InstallOptionsParsed, String> {
    let mut options = InstallOptionsParsed {
        deploy_root: None,
        source: InstallSourceKind::Path(PathBuf::from(".")),
        version: None,
        skip_migrate: false,
        no_switch: false,
        restart_cmd: None,
        dry_run: false,
    };
    let mut has_source = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--deploy-root" => {
                index += 1;
                options.deploy_root =
                    Some(PathBuf::from(next_value(&args, index, "--deploy-root")?));
            }
            "--tarball" => {
                index += 1;
                let path = PathBuf::from(next_value(&args, index, "--tarball")?);
                options.source = InstallSourceKind::Tarball(path);
                has_source = true;
            }
            "--path" => {
                index += 1;
                let path = PathBuf::from(next_value(&args, index, "--path")?);
                options.source = InstallSourceKind::Path(path);
                has_source = true;
            }
            "--version" => {
                index += 1;
                options.version = Some(next_value(&args, index, "--version")?);
            }
            "--skip-migrate" => options.skip_migrate = true,
            "--no-switch" => options.no_switch = true,
            "--restart-cmd" => {
                index += 1;
                options.restart_cmd = Some(next_value(&args, index, "--restart-cmd")?);
            }
            "--dry-run" => options.dry_run = true,
            flag if flag.starts_with("--deploy-root=") => {
                options.deploy_root =
                    Some(PathBuf::from(flag["--deploy-root=".len()..].to_owned()));
            }
            flag if flag.starts_with("--tarball=") => {
                options.source = InstallSourceKind::Tarball(PathBuf::from(
                    flag["--tarball=".len()..].to_owned(),
                ));
                has_source = true;
            }
            flag if flag.starts_with("--path=") => {
                options.source =
                    InstallSourceKind::Path(PathBuf::from(flag["--path=".len()..].to_owned()));
                has_source = true;
            }
            flag if flag.starts_with("--version=") => {
                options.version = Some(flag["--version=".len()..].to_owned());
            }
            flag if flag.starts_with("--restart-cmd=") => {
                options.restart_cmd = Some(flag["--restart-cmd=".len()..].to_owned());
            }
            flag => return Err(format!("unknown release:install option `{flag}`")),
        }
        index += 1;
    }
    if !has_source {
        return Err("release:install requires --tarball <path> or --path <dir>".to_owned());
    }
    Ok(options)
}

fn parse_rollback_args(args: Vec<String>) -> Result<RollbackOptionsParsed, String> {
    let mut options = RollbackOptionsParsed {
        deploy_root: None,
        to: None,
        steps: 1,
        restart_cmd: None,
        skip_restart: false,
        dry_run: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--deploy-root" => {
                index += 1;
                options.deploy_root =
                    Some(PathBuf::from(next_value(&args, index, "--deploy-root")?));
            }
            "--to" => {
                index += 1;
                options.to = Some(next_value(&args, index, "--to")?);
            }
            "--steps" => {
                index += 1;
                options.steps = parse_steps(&next_value(&args, index, "--steps")?)?;
            }
            "--restart-cmd" => {
                index += 1;
                options.restart_cmd = Some(next_value(&args, index, "--restart-cmd")?);
            }
            "--skip-restart" => options.skip_restart = true,
            "--dry-run" => options.dry_run = true,
            flag if flag.starts_with("--deploy-root=") => {
                options.deploy_root =
                    Some(PathBuf::from(flag["--deploy-root=".len()..].to_owned()));
            }
            flag if flag.starts_with("--to=") => {
                options.to = Some(flag["--to=".len()..].to_owned());
            }
            flag if flag.starts_with("--steps=") => {
                options.steps = parse_steps(&flag["--steps=".len()..])?;
            }
            flag if flag.starts_with("--restart-cmd=") => {
                options.restart_cmd = Some(flag["--restart-cmd=".len()..].to_owned());
            }
            flag => return Err(format!("unknown release:rollback option `{flag}`")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_status_args(args: Vec<String>) -> Result<StatusOptions, String> {
    let mut options = StatusOptions {
        deploy_root: None,
        json: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--deploy-root" => {
                index += 1;
                options.deploy_root =
                    Some(PathBuf::from(next_value(&args, index, "--deploy-root")?));
            }
            "--json" => options.json = true,
            flag if flag.starts_with("--deploy-root=") => {
                options.deploy_root =
                    Some(PathBuf::from(flag["--deploy-root=".len()..].to_owned()));
            }
            flag => return Err(format!("unknown release:status option `{flag}`")),
        }
        index += 1;
    }
    Ok(options)
}

fn next_value(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_steps(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|steps| *steps > 0)
        .ok_or_else(|| "rollback steps must be a positive integer".to_owned())
}

fn resolve_deploy_root(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    env::var("PHOENIX_DEPLOY_ROOT")
        .map(PathBuf::from)
        .map_err(|_| {
            "deploy root is required; pass --deploy-root or set PHOENIX_DEPLOY_ROOT".to_owned()
        })
}

fn resolve_install_version(
    explicit: Option<&str>,
    source: &InstallSourceKind,
) -> Result<String, String> {
    if let Some(version) = explicit {
        return Ok(version.to_owned());
    }
    match source {
        InstallSourceKind::Path(path) => manifest_version_at(path),
        InstallSourceKind::Tarball(path) => {
            let temp = env::temp_dir().join(format!(
                "px-release-install-{}-{}",
                std::process::id(),
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("tarball")
            ));
            if temp.exists() {
                fs::remove_dir_all(&temp).map_err(|e| e.to_string())?;
            }
            extract_tarball(path, &temp).map_err(release_err)?;
            let version = manifest_version_at(&temp)?;
            let _ = fs::remove_dir_all(&temp);
            Ok(version)
        }
    }
}

fn manifest_version_at(path: &Path) -> Result<String, String> {
    let manifest_path = path.join("manifest.toml");
    if !manifest_path.is_file() {
        return Err(format!(
            "manifest missing at {}; pass --version explicitly",
            manifest_path.display()
        ));
    }
    let text = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("version = ") {
            return parse_toml_string_value(line.strip_prefix("version = ").unwrap_or(line));
        }
    }
    Err(format!(
        "could not read version from {}; pass --version explicitly",
        manifest_path.display()
    ))
}

fn read_cargo_package(root: &Path) -> Result<CargoPackage, String> {
    let cargo_toml = root.join("Cargo.toml");
    let text = fs::read_to_string(&cargo_toml)
        .map_err(|error| format!("failed to read {}: {error}", cargo_toml.display()))?;
    let mut in_package = false;
    let mut name = None;
    let mut version = None;
    let mut default_run = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name = ") {
            name = Some(parse_toml_string_value(value)?);
        } else if let Some(value) = trimmed.strip_prefix("version = ") {
            version = Some(parse_toml_string_value(value)?);
        } else if let Some(value) = trimmed.strip_prefix("default-run = ") {
            default_run = Some(parse_toml_string_value(value)?);
        }
    }
    Ok(CargoPackage {
        name: name.ok_or("Cargo.toml [package] name is missing")?,
        version: version.ok_or("Cargo.toml [package] version is missing")?,
        default_run,
    })
}

fn parse_toml_string_value(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        Ok(raw[1..raw.len() - 1].to_owned())
    } else {
        Err(format!("expected quoted TOML string, got `{raw}`"))
    }
}

fn release_profile_dir(root: &Path, target: Option<&str>) -> PathBuf {
    match target {
        Some(triple) => root.join("target").join(triple).join("release"),
        None => root.join("target/release"),
    }
}

fn ensure_dir(path: PathBuf) -> Result<PathBuf, String> {
    if !path.exists() {
        fs::create_dir_all(&path)
            .map_err(|error| format!("failed to create directory {}: {error}", path.display()))?;
    }
    Ok(path)
}

fn run_command(program: &str, args: &[&str], cwd: &Path, label: &str) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|error| format!("failed to start `{label}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{label}` exited with {status}"))
    }
}

fn run_command_strings(
    program: &str,
    args: &[String],
    cwd: &Path,
    label: &str,
) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|error| format!("failed to start `{label}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{label}` exited with {status}"))
    }
}

fn host_triple() -> String {
    rustc_metadata()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("host: "))
                .map(|line| line["host: ".len()..].trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn rustc_version() -> Option<String> {
    rustc_metadata().and_then(|text| {
        text.lines()
            .find(|line| line.starts_with("release: "))
            .map(|line| line["release: ".len()..].trim().to_owned())
    })
}

fn rustc_metadata() -> Option<String> {
    Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
}

fn git_revision(root: &Path) -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn read_json_str_field(path: &Path, field: &str) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get(field)
        .and_then(|entry| entry.as_str())
        .map(ToOwned::to_owned)
}

fn print_status_human(deploy_root: &Path, snapshot: &DeployStatus) {
    println!("DEPLOY_ROOT {}", deploy_root.display());
    println!(
        "CURRENT {}",
        snapshot.current_version.as_deref().unwrap_or("(none)")
    );
    println!(
        "PREVIOUS {}",
        snapshot.previous_version.as_deref().unwrap_or("(none)")
    );
    println!("LOCKED={}", snapshot.locked);
    if snapshot.releases.is_empty() {
        println!("RELEASES (none)");
        return;
    }
    println!("RELEASES");
    for release in &snapshot.releases {
        println!(
            "  {} application={} migrations={} created_at={}",
            release.version,
            release.application.as_deref().unwrap_or("-"),
            release
                .migration_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            release.created_at.as_deref().unwrap_or("-"),
        );
    }
}

fn print_status_json(snapshot: &DeployStatus) {
    let releases: Vec<serde_json::Value> = snapshot
        .releases
        .iter()
        .map(|release| {
            serde_json::json!({
                "version": release.version,
                "application": release.application,
                "migration_count": release.migration_count,
                "created_at": release.created_at,
            })
        })
        .collect();
    let value = serde_json::json!({
        "current_version": snapshot.current_version,
        "previous_version": snapshot.previous_version,
        "locked": snapshot.locked,
        "releases": releases,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

fn release_err(error: ReleaseError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_build_flags() {
        let options = parse_build_args(vec![
            "--version".into(),
            "1.2.3".into(),
            "--tarball".into(),
            "--skip-npm".into(),
        ])
        .unwrap();
        assert_eq!(options.version.as_deref(), Some("1.2.3"));
        assert!(options.tarball);
        assert!(options.skip_npm);
    }

    #[test]
    fn parse_install_requires_source() {
        assert!(parse_install_args(vec![]).is_err());
    }

    #[test]
    fn parse_toml_string() {
        assert_eq!(parse_toml_string_value("\"demo\"").unwrap(), "demo");
    }
}
