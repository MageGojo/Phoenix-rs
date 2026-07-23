//! Release tarball packing and staging.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Write, copy},
    path::{Path, PathBuf},
    time::SystemTime,
};

use flate2::{Compression, write::GzEncoder};
use tar::{Builder, EntryType};

use crate::{
    checksum::sha256_file,
    error::ReleaseError,
    manifest::{
        ApplicationMeta, AssetsMeta, BuildMeta, MANIFEST_FILE, MANIFEST_SCHEMA_VERSION,
        MigrationsMeta, ReleaseManifest,
    },
};

/// Options controlling staging layout and manifest metadata.
#[derive(Clone, Debug)]
pub struct PackOptions {
    pub version: String,
    pub app_name: String,
    pub binary_name: String,
    pub target_triple: String,
    pub staging_dir: PathBuf,
    pub git_revision: Option<String>,
    pub client_manifest: Option<String>,
    pub ssr_manifest: Option<String>,
    pub contract_hash: Option<String>,
    pub rustc_version: Option<String>,
    pub profile: Option<String>,
    pub npm_build: Option<String>,
}

/// Source paths supplied by the caller (typically the CLI build step).
#[derive(Clone, Debug)]
pub struct StagingSources {
    pub binary: PathBuf,
    pub phoenix_manage: PathBuf,
    pub public_assets: PathBuf,
    pub public_ssr: PathBuf,
    pub config: PathBuf,
    pub migrations: PathBuf,
}

/// Populate `options.staging_dir` from `sources` and return the manifest.
pub fn write_staging(
    options: &PackOptions,
    sources: &StagingSources,
) -> Result<ReleaseManifest, ReleaseError> {
    let staging = &options.staging_dir;
    if staging.exists() {
        fs::remove_dir_all(staging).map_err(|source| ReleaseError::io(staging, source))?;
    }

    let bin_dir = staging.join("bin");
    let public_dir = staging.join("public");
    let config_dir = staging.join("config");
    let migrations_dir = staging.join("database/migrations");

    fs::create_dir_all(&bin_dir).map_err(|source| ReleaseError::io(&bin_dir, source))?;
    fs::create_dir_all(&public_dir).map_err(|source| ReleaseError::io(&public_dir, source))?;
    fs::create_dir_all(&config_dir).map_err(|source| ReleaseError::io(&config_dir, source))?;
    fs::create_dir_all(&migrations_dir)
        .map_err(|source| ReleaseError::io(&migrations_dir, source))?;

    let app_binary_dest = bin_dir.join(&options.binary_name);
    copy_file(&sources.binary, &app_binary_dest)?;
    #[cfg(unix)]
    set_executable(&app_binary_dest)?;

    let manage_dest = bin_dir.join("phoenix-manage");
    copy_file(&sources.phoenix_manage, &manage_dest)?;
    #[cfg(unix)]
    set_executable(&manage_dest)?;

    copy_tree(&sources.public_assets, &public_dir)?;
    copy_tree(&sources.public_ssr, &public_dir)?;
    copy_tree(&sources.config, &config_dir)?;

    let migration_count = copy_tree(&sources.migrations, &migrations_dir)?;

    let mut checksums = BTreeMap::new();
    collect_checksums(staging, staging, &mut checksums)?;

    let manifest = ReleaseManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        version: options.version.clone(),
        created_at: rfc3339_now()?,
        git_revision: options.git_revision.clone(),
        application: ApplicationMeta {
            name: options.app_name.clone(),
            binary: options.binary_name.clone(),
            target_triple: options.target_triple.clone(),
        },
        assets: AssetsMeta {
            client_manifest: options.client_manifest.clone(),
            ssr_manifest: options.ssr_manifest.clone(),
            contract_hash: options.contract_hash.clone(),
        },
        migrations: MigrationsMeta {
            included: migration_count > 0,
            count: migration_count,
        },
        checksums,
        build: BuildMeta {
            rustc_version: options.rustc_version.clone(),
            profile: options.profile.clone(),
            npm_build: options.npm_build.clone(),
        },
    };

    manifest.write_to(staging)?;
    // Do not checksum manifest.toml itself — embedding its own digest is unstable.
    Ok(manifest)
}

/// Create a `.tar.gz` from `staging_dir` and return its SHA-256 digest.
pub fn create_tarball(
    staging_dir: impl AsRef<Path>,
    output_gz_path: impl AsRef<Path>,
) -> Result<String, ReleaseError> {
    let staging_dir = staging_dir.as_ref();
    let output_gz_path = output_gz_path.as_ref();

    if let Some(parent) = output_gz_path.parent() {
        fs::create_dir_all(parent).map_err(|source| ReleaseError::io(parent, source))?;
    }

    let file =
        File::create(output_gz_path).map_err(|source| ReleaseError::io(output_gz_path, source))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);

    append_dir_to_tar(staging_dir, staging_dir, &mut builder)?;

    let encoder = builder
        .into_inner()
        .map_err(|err| ReleaseError::Manifest(format!("failed to finalize tarball: {err}")))?;
    encoder
        .finish()
        .map_err(|err| ReleaseError::Manifest(format!("failed to finish gzip: {err}")))?;

    sha256_file(output_gz_path)
}

/// Extract a `.tar.gz` tarball into `dest_dir`.
pub fn extract_tarball(
    tarball: impl AsRef<Path>,
    dest_dir: impl AsRef<Path>,
) -> Result<(), ReleaseError> {
    let tarball = tarball.as_ref();
    let dest_dir = dest_dir.as_ref();

    fs::create_dir_all(dest_dir).map_err(|source| ReleaseError::io(dest_dir, source))?;

    let file = File::open(tarball).map_err(|source| ReleaseError::io(tarball, source))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|source| ReleaseError::io(tarball, source))?
    {
        let mut entry = entry.map_err(|source| ReleaseError::io(tarball, source))?;
        let path = entry
            .path()
            .map_err(|source| ReleaseError::io(tarball, source))?
            .into_owned();
        let out_path = dest_dir.join(&path);

        if entry.header().entry_type() == EntryType::Directory {
            fs::create_dir_all(&out_path).map_err(|source| ReleaseError::io(&out_path, source))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|source| ReleaseError::io(parent, source))?;
            }
            let mut out_file =
                File::create(&out_path).map_err(|source| ReleaseError::io(&out_path, source))?;
            copy(&mut entry, &mut out_file)
                .map_err(|source| ReleaseError::io(&out_path, source))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mode) = entry.header().mode() {
                    let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode));
                }
            }
        }
    }

    Ok(())
}

fn rfc3339_now() -> Result<String, ReleaseError> {
    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| ReleaseError::Manifest(err.to_string()))?;
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    let days = secs / 86_400;
    let day_secs = secs % 86_400;
    let hours = day_secs / 3_600;
    let minutes = (day_secs % 3_600) / 60;
    let seconds = day_secs % 60;

    let (year, month, day) = unix_days_to_ymd(days);
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{nanos:09}Z"
    ))
}

fn unix_days_to_ymd(mut days: u64) -> (i32, u32, u32) {
    days += 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i32::try_from(yoe).unwrap_or(0) + i32::try_from(era * 400).unwrap_or(0);
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

fn copy_file(from: &Path, to: &Path) -> Result<(), ReleaseError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|source| ReleaseError::io(parent, source))?;
    }
    fs::copy(from, to).map_err(|source| ReleaseError::io(to, source))?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), ReleaseError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|source| ReleaseError::io(path, source))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|source| ReleaseError::io(path, source))
}

fn copy_tree(from: &Path, to: &Path) -> Result<u32, ReleaseError> {
    if !from.exists() {
        return Ok(0);
    }

    let mut count = 0_u32;
    if from.is_file() {
        copy_file(from, to)?;
        return Ok(1);
    }

    for entry in fs::read_dir(from).map_err(|source| ReleaseError::io(from, source))? {
        let entry = entry.map_err(|source| ReleaseError::io(from, source))?;
        let src_path = entry.path();
        let dest_path = to.join(entry.file_name());
        if src_path.is_dir() {
            fs::create_dir_all(&dest_path)
                .map_err(|source| ReleaseError::io(&dest_path, source))?;
            count += copy_tree(&src_path, &dest_path)?;
        } else {
            copy_file(&src_path, &dest_path)?;
            count += 1;
        }
    }
    Ok(count)
}

fn collect_checksums(
    base: &Path,
    current: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<(), ReleaseError> {
    if current.is_file() {
        let relative = current
            .strip_prefix(base)
            .map_err(|err| ReleaseError::Manifest(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative == MANIFEST_FILE {
            return Ok(());
        }
        out.insert(relative, sha256_file(current)?);
        return Ok(());
    }

    for entry in fs::read_dir(current).map_err(|source| ReleaseError::io(current, source))? {
        let entry = entry.map_err(|source| ReleaseError::io(current, source))?;
        collect_checksums(base, &entry.path(), out)?;
    }
    Ok(())
}

fn append_dir_to_tar(
    base: &Path,
    current: &Path,
    builder: &mut Builder<impl Write>,
) -> Result<(), ReleaseError> {
    if current.is_file() {
        let relative = current
            .strip_prefix(base)
            .map_err(|err| ReleaseError::Manifest(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        builder
            .append_path_with_name(current, &relative)
            .map_err(|source| ReleaseError::io(current, source))?;
        return Ok(());
    }

    for entry in fs::read_dir(current).map_err(|source| ReleaseError::io(current, source))? {
        let entry = entry.map_err(|source| ReleaseError::io(current, source))?;
        append_dir_to_tar(base, &entry.path(), builder)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn staging_and_tarball_roundtrip() {
        let dir = tempdir().unwrap();
        let binary = dir.path().join("app");
        let manage = dir.path().join("manage");
        fs::write(&binary, b"bin").unwrap();
        fs::write(&manage, b"manage").unwrap();

        let public = dir.path().join("public");
        fs::create_dir_all(&public).unwrap();

        let staging = dir.path().join("staging");
        let manifest = write_staging(
            &PackOptions {
                version: "0.1.0".into(),
                app_name: "demo".into(),
                binary_name: "demo".into(),
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
                public_assets: public.clone(),
                public_ssr: public,
                config: dir.path().join("config"),
                migrations: dir.path().join("migrations"),
            },
        )
        .unwrap();

        assert!(staging.join("bin/demo").is_file());
        assert!(manifest.checksums.contains_key("bin/demo"));

        let tarball = dir.path().join("demo.tar.gz");
        create_tarball(&staging, &tarball).unwrap();
        let extracted = dir.path().join("extracted");
        extract_tarball(&tarball, &extracted).unwrap();
        assert!(extracted.join("manifest.toml").is_file());
        assert!(extracted.join("bin/demo").is_file());
    }
}
