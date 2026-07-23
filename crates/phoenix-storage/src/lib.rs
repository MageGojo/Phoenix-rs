//! Local disk and future object-storage drivers for Phoenix uploads.
//!
//! See `docs/TESTING_AND_STORAGE.md`.

#![forbid(unsafe_code)]

use std::{
    future::Future,
    path::{Component, Path, PathBuf},
};

use bytes::Bytes;
use rand::Rng;
use thiserror::Error;
use tokio::fs;

/// Errors produced by storage drivers.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The object key is empty, absolute, contains `..`, or uses disallowed characters.
    #[error("invalid storage key: {0}")]
    InvalidKey(String),
    /// The resolved path escapes the configured storage root.
    #[error("storage path escapes root for key: {0}")]
    PathEscape(String),
    /// No object exists for the given key.
    #[error("object not found: {0}")]
    NotFound(String),
    /// Underlying filesystem failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Object storage used by upload and download helpers.
pub trait Storage: Send + Sync {
    /// Persist bytes under `key`.
    fn put(&self, key: &str, bytes: Bytes)
    -> impl Future<Output = Result<(), StorageError>> + Send;

    /// Read bytes previously stored under `key`.
    fn get(&self, key: &str) -> impl Future<Output = Result<Bytes, StorageError>> + Send;

    /// Delete the object at `key`. Missing keys succeed.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StorageError>> + Send;

    /// Return whether an object exists at `key`.
    fn exists(&self, key: &str) -> impl Future<Output = Result<bool, StorageError>> + Send;

    /// Resolve the on-disk path for `key` without reading the object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidKey`] or [`StorageError::PathEscape`] when the key is unsafe.
    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError>;
}

/// Filesystem storage rooted at a single directory.
#[derive(Clone, Debug)]
pub struct LocalDisk {
    root: PathBuf,
}

impl LocalDisk {
    /// Create a driver rooted at `root`, creating the directory when missing.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the root cannot be created or canonicalized.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        Ok(Self { root })
    }

    /// Borrow the canonical storage root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Store raw bytes under `key` (same as [`Storage::put`]).
    ///
    /// Useful for multipart helpers that already hold a buffered field body.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Storage::put`].
    pub async fn store_bytes(&self, key: &str, bytes: Bytes) -> Result<(), StorageError> {
        self.put(key, bytes).await
    }

    fn resolve(&self, key: &str) -> Result<PathBuf, StorageError> {
        let relative = sanitize_key(key)?;
        let candidate = self.root.join(&relative);
        ensure_under_root(&self.root, &candidate, key)?;
        Ok(candidate)
    }
}

impl Storage for LocalDisk {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), StorageError> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
            ensure_under_root(&self.root, parent, key)?;
        }

        let mut suffix = [0_u8; 8];
        rand::rng().fill_bytes(&mut suffix);
        let tmp_name = format!(
            ".{}.tmp-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("upload"),
            hex_suffix(&suffix)
        );
        let tmp_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(tmp_name);

        let write_result = async {
            fs::write(&tmp_path, &bytes).await?;
            ensure_under_root(&self.root, &tmp_path, key)?;
            fs::rename(&tmp_path, &path).await?;
            ensure_under_root(&self.root, &path, key)?;
            Ok(())
        }
        .await;

        if write_result.is_err() {
            let _ = fs::remove_file(&tmp_path).await;
        }
        write_result
    }

    async fn get(&self, key: &str) -> Result<Bytes, StorageError> {
        let path = self.resolve(key)?;
        match fs::read(&path).await {
            Ok(bytes) => {
                ensure_under_root(&self.root, &path, key)?;
                Ok(Bytes::from(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(key.to_owned()))
            }
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.resolve(key)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let path = self.resolve(key)?;
        Ok(fs::try_exists(&path).await?)
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        self.resolve(key)
    }
}

/// Normalize and validate a storage key into a relative path.
///
/// # Errors
///
/// Rejects empty keys, absolute paths, `..`, NUL/control characters, and Windows drive letters.
pub fn sanitize_key(key: &str) -> Result<PathBuf, StorageError> {
    if key.is_empty() {
        return Err(StorageError::InvalidKey("key must not be empty".into()));
    }
    if key.contains('\0') || key.chars().any(char::is_control) {
        return Err(StorageError::InvalidKey(
            "key must not contain NUL or control characters".into(),
        ));
    }
    if looks_absolute(key) {
        return Err(StorageError::InvalidKey(
            "key must be a relative path".into(),
        ));
    }

    let normalized = key.replace('\\', "/");
    let path = Path::new(&normalized);
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() || part == "." {
                    continue;
                }
                if part == ".." {
                    return Err(StorageError::InvalidKey("key must not contain '..'".into()));
                }
                cleaned.push(part.as_ref());
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(StorageError::InvalidKey(
                    "key must be a relative path".into(),
                ));
            }
            Component::ParentDir => {
                return Err(StorageError::InvalidKey("key must not contain '..'".into()));
            }
        }
    }

    if cleaned.as_os_str().is_empty() {
        return Err(StorageError::InvalidKey(
            "key must resolve to a non-empty relative path".into(),
        ));
    }
    Ok(cleaned)
}

fn looks_absolute(key: &str) -> bool {
    let trimmed = key.trim_start();
    trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || (trimmed
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
            && trimmed.get(1..2) == Some(":"))
}

fn ensure_under_root(root: &Path, path: &Path, key: &str) -> Result<(), StorageError> {
    let check =
        if path.exists() {
            path.canonicalize().map_err(StorageError::Io)?
        } else if let Some(parent) = path.parent() {
            if parent.as_os_str().is_empty() {
                root.to_path_buf()
            } else if parent.exists() {
                let parent = parent.canonicalize().map_err(StorageError::Io)?;
                parent.join(path.file_name().ok_or_else(|| {
                    StorageError::InvalidKey("key must include a file name".into())
                })?)
            } else {
                // Parent is still under construction; walk existing ancestors.
                let mut ancestor = parent.to_path_buf();
                while !ancestor.exists() {
                    if !ancestor.pop() {
                        break;
                    }
                }
                if ancestor.exists() {
                    let ancestor = ancestor.canonicalize().map_err(StorageError::Io)?;
                    if !is_path_within(&ancestor, root) {
                        return Err(StorageError::PathEscape(key.to_owned()));
                    }
                }
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };

    if !is_path_within(&check, root) {
        return Err(StorageError::PathEscape(key.to_owned()));
    }
    Ok(())
}

fn is_path_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn hex_suffix(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "phoenix-storage-{label}-{}-{id}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn put_get_delete_round_trip() {
        let root = temp_root("round-trip");
        let storage = LocalDisk::new(&root).unwrap();

        storage
            .put("avatars/user.txt", Bytes::from_static(b"hello"))
            .await
            .unwrap();
        assert!(storage.exists("avatars/user.txt").await.unwrap());
        assert_eq!(
            storage.get("avatars/user.txt").await.unwrap().as_ref(),
            b"hello"
        );

        let path = storage.path_for("avatars/user.txt").unwrap();
        assert!(path.starts_with(storage.root()));
        assert!(path.is_file());

        storage.delete("avatars/user.txt").await.unwrap();
        assert!(!storage.exists("avatars/user.txt").await.unwrap());
        assert!(matches!(
            storage.get("avatars/user.txt").await,
            Err(StorageError::NotFound(_))
        ));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn store_bytes_writes_atomically() {
        let root = temp_root("store-bytes");
        let storage = LocalDisk::new(&root).unwrap();
        storage
            .store_bytes("docs/a.bin", Bytes::from_static(b"payload"))
            .await
            .unwrap();
        assert_eq!(
            storage.get("docs/a.bin").await.unwrap().as_ref(),
            b"payload"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sanitize_key_rejects_traversal_and_absolute_paths() {
        assert!(sanitize_key("../etc/passwd").is_err());
        assert!(sanitize_key("/etc/passwd").is_err());
        assert!(sanitize_key("C:\\Windows\\system32").is_err());
        assert!(sanitize_key("avatars/\0evil").is_err());
        assert!(sanitize_key("avatars/\nevil").is_err());
        assert!(sanitize_key("").is_err());
        assert!(sanitize_key(".").is_err());
        assert_eq!(
            sanitize_key("avatars/./user.txt").unwrap(),
            PathBuf::from("avatars/user.txt")
        );
    }

    #[tokio::test]
    async fn resolve_rejects_path_traversal_keys() {
        let root = temp_root("traversal");
        let storage = LocalDisk::new(&root).unwrap();
        assert!(matches!(
            storage.put("../escape.txt", Bytes::from_static(b"x")).await,
            Err(StorageError::InvalidKey(_))
        ));
        assert!(matches!(
            storage.path_for("/abs.txt"),
            Err(StorageError::InvalidKey(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape_on_read() {
        let root = temp_root("symlink-root");
        let outside = temp_root("symlink-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("link.txt")).unwrap();

        let storage = LocalDisk::new(&root).unwrap();
        let result = storage.get("link.txt").await;
        assert!(
            matches!(result, Err(StorageError::PathEscape(_))),
            "expected path escape, got {result:?}"
        );

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
