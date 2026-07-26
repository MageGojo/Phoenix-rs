//! S3-compatible object storage driver ([`S3Disk`]).
//!
//! Implements the [`Storage`](crate::Storage) trait against Amazon S3 and
//! S3-compatible services (`MinIO`, Alibaba Cloud OSS, …) using self-implemented
//! AWS Signature V4 (see [`crate::sigv4`]) over the pluggable
//! [`S3Http`](crate::S3Http) transport. It also mints presigned GET/PUT URLs
//! for browser direct-upload and CDN origin pulls.
//!
//! ```no_run
//! use std::time::Duration;
//! use bytes::Bytes;
//! use phoenix_storage::{Addressing, S3Config, S3Disk, Storage};
//!
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let config = S3Config::new(
//!     "http://127.0.0.1:9000", // MinIO endpoint
//!     "us-east-1",
//!     "uploads",
//!     "minioadmin",
//!     "minioadmin",
//! )
//! .with_addressing(Addressing::Path); // MinIO likes path-style
//!
//! let disk = S3Disk::new(config)?;
//! disk.put("avatars/user.png", Bytes::from_static(b"...")).await?;
//! let url = disk.presigned_get_url("avatars/user.png", Duration::from_secs(900))?;
//! # let _ = url;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, StatusCode};

use crate::secret::Secret;
use crate::sigv4::{self, SigV4};
use crate::transport::{HyperS3Http, S3Http, S3Request, S3Response};
use crate::{Storage, StorageError, sanitize_key};

/// S3 request-addressing style.
///
/// - [`Addressing::Path`] — `endpoint/bucket/key`; the safe default for `MinIO`
///   and custom endpoints that do not serve wildcard virtual-host subdomains.
/// - [`Addressing::VirtualHost`] — `bucket.endpoint/key`; how Amazon S3 and
///   Alibaba Cloud OSS address buckets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Addressing {
    /// `scheme://endpoint-host/bucket/key`.
    #[default]
    Path,
    /// `scheme://bucket.endpoint-host/key`.
    VirtualHost,
}

/// Connection + credential configuration for [`S3Disk`].
///
/// The `secret_key` is stored as a redacted [`Secret`], so a `Debug` of this
/// config (or of the [`S3Disk`] holding it) never leaks the key.
#[derive(Clone, Debug)]
pub struct S3Config {
    /// Endpoint origin, e.g. `https://s3.amazonaws.com`,
    /// `https://oss-cn-hangzhou.aliyuncs.com`, or `http://127.0.0.1:9000`.
    pub endpoint: String,
    /// Signing region, e.g. `us-east-1`. `MinIO` commonly uses `us-east-1`.
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Access key id (not secret).
    pub access_key: String,
    /// Secret access key (redacted in `Debug`, zeroized on drop).
    pub secret_key: Secret,
    /// Path-style vs virtual-host addressing.
    pub addressing: Addressing,
}

impl S3Config {
    /// Build a config with the default [`Addressing::Path`] style.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        region: impl Into<String>,
        bucket: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<Secret>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            region: region.into(),
            bucket: bucket.into(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            addressing: Addressing::Path,
        }
    }

    /// Set the addressing style (builder-style).
    #[must_use]
    pub fn with_addressing(mut self, addressing: Addressing) -> Self {
        self.addressing = addressing;
        self
    }
}

/// S3-compatible object storage driver.
#[derive(Clone)]
pub struct S3Disk {
    config: S3Config,
    scheme: String,
    endpoint_authority: String,
    service: &'static str,
    http: Arc<dyn S3Http>,
}

impl fmt::Debug for S3Disk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Disk")
            .field("config", &self.config)
            .field("scheme", &self.scheme)
            .field("endpoint", &self.endpoint_authority)
            .finish_non_exhaustive()
    }
}

impl S3Disk {
    /// Create a driver using the default hyper + rustls transport.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] when `config.endpoint` is not a valid
    /// `http`/`https` origin.
    pub fn new(config: S3Config) -> Result<Self, StorageError> {
        Self::with_transport(config, Arc::new(HyperS3Http::new()))
    }

    /// Create a driver with a custom [`S3Http`] transport (tests, proxies).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] when `config.endpoint` is not a valid
    /// `http`/`https` origin.
    pub fn with_transport(config: S3Config, http: Arc<dyn S3Http>) -> Result<Self, StorageError> {
        let (scheme, endpoint_authority) = parse_endpoint(&config.endpoint)?;
        Ok(Self {
            config,
            scheme,
            endpoint_authority,
            service: "s3",
            http,
        })
    }

    /// Presigned URL for downloading `key`, valid for `expires`.
    ///
    /// Hand this to a browser or CDN so it can `GET` the object directly
    /// without the secret key. `expires` is clamped to S3's 1s–7d range.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidKey`] / [`StorageError::PathEscape`] for
    /// unsafe keys.
    pub fn presigned_get_url(&self, key: &str, expires: Duration) -> Result<String, StorageError> {
        self.presign(&Method::GET, key, expires, sigv4::now_unix())
    }

    /// Presigned URL for uploading to `key` with `PUT`, valid for `expires`.
    ///
    /// Hand this to a browser for direct upload. `expires` is clamped to S3's
    /// 1s–7d range.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidKey`] / [`StorageError::PathEscape`] for
    /// unsafe keys.
    pub fn presigned_put_url(&self, key: &str, expires: Duration) -> Result<String, StorageError> {
        self.presign(&Method::PUT, key, expires, sigv4::now_unix())
    }

    /// Presign at a fixed clock (kept private; `now`-injection is for tests).
    fn presign(
        &self,
        method: &Method,
        key: &str,
        expires: Duration,
        now: u64,
    ) -> Result<String, StorageError> {
        let key = sanitized_key(key)?;
        let (host, canonical_uri) = self.resolve_target(&key);
        let (amz_date, date_stamp) = sigv4::format_timestamp(now);
        let expires_secs = expires.as_secs().clamp(1, 604_800);
        let signer = self.signer(&amz_date, &date_stamp);
        let presigned = signer.presign(method.as_str(), &canonical_uri, &host, expires_secs);
        Ok(format!(
            "{}://{host}{canonical_uri}?{}",
            self.scheme, presigned.query
        ))
    }

    /// Compute the `Host` authority and already-encoded canonical URI for a
    /// sanitized object key, honoring the addressing style.
    fn resolve_target(&self, sanitized_key: &str) -> (String, String) {
        match self.config.addressing {
            Addressing::Path => {
                let host = self.endpoint_authority.clone();
                let uri =
                    sigv4::uri_encode(&format!("/{}/{sanitized_key}", self.config.bucket), true);
                (host, uri)
            }
            Addressing::VirtualHost => {
                let host = match self.endpoint_authority.split_once(':') {
                    Some((host, port)) => format!("{}.{host}:{port}", self.config.bucket),
                    None => format!("{}.{}", self.config.bucket, self.endpoint_authority),
                };
                let uri = sigv4::uri_encode(&format!("/{sanitized_key}"), true);
                (host, uri)
            }
        }
    }

    fn signer<'a>(&'a self, amz_date: &'a str, date_stamp: &'a str) -> SigV4<'a> {
        SigV4 {
            access_key: &self.config.access_key,
            secret_key: self.config.secret_key.expose(),
            region: &self.config.region,
            service: self.service,
            amz_date,
            date_stamp,
        }
    }

    /// Sign and send one request for a sanitized key.
    async fn send_signed(
        &self,
        method: &Method,
        sanitized_key: &str,
        body: Bytes,
        extra_headers: Vec<(String, String)>,
    ) -> Result<S3Response, StorageError> {
        let (host, canonical_uri) = self.resolve_target(sanitized_key);
        let (amz_date, date_stamp) = sigv4::format_timestamp(sigv4::now_unix());
        let payload_hash = sigv4::sha256_hex(&body);

        // Headers included in the signature. `host` is signed here but sent by
        // the transport, so it appears exactly once on the wire.
        let signed_headers = [
            ("host".to_owned(), host.clone()),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), amz_date.clone()),
        ];
        let signature = self.signer(&amz_date, &date_stamp).sign_headers(
            method.as_str(),
            &canonical_uri,
            "",
            &signed_headers,
            &payload_hash,
        );

        let mut headers = vec![
            ("authorization".to_owned(), signature.authorization),
            ("x-amz-content-sha256".to_owned(), payload_hash),
            ("x-amz-date".to_owned(), amz_date),
        ];
        headers.extend(extra_headers);

        let request = S3Request {
            method: method.clone(),
            url: format!("{}://{host}{canonical_uri}", self.scheme),
            headers,
            body,
        };
        self.http.send(request).await
    }
}

impl Storage for S3Disk {
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), StorageError> {
        let key = sanitized_key(key)?;
        let extra = vec![(
            "content-type".to_owned(),
            "application/octet-stream".to_owned(),
        )];
        let response = self.send_signed(&Method::PUT, &key, bytes, extra).await?;
        ensure_success(&response, &key)
    }

    async fn get(&self, key: &str) -> Result<Bytes, StorageError> {
        let sanitized = sanitized_key(key)?;
        let response = self
            .send_signed(&Method::GET, &sanitized, Bytes::new(), Vec::new())
            .await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(StorageError::NotFound(key.to_owned()));
        }
        ensure_success(&response, key)?;
        Ok(response.body)
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let sanitized = sanitized_key(key)?;
        let response = self
            .send_signed(&Method::DELETE, &sanitized, Bytes::new(), Vec::new())
            .await?;
        // S3 returns 204 for delete and treats missing keys as success.
        if response.status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        ensure_success(&response, key)
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let sanitized = sanitized_key(key)?;
        let response = self
            .send_signed(&Method::HEAD, &sanitized, Bytes::new(), Vec::new())
            .await?;
        match response.status {
            StatusCode::NOT_FOUND => Ok(false),
            status if status.is_success() => Ok(true),
            _ => Err(backend_error(&response, key)),
        }
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        // Validate the key so callers get the same rejection as `LocalDisk`,
        // then make clear an object store has no local filesystem path.
        let _ = sanitize_key(key)?;
        Err(StorageError::Unsupported(format!(
            "S3Disk has no local path for `{key}`; use presigned_get_url"
        )))
    }
}

/// Validate `key` with the shared [`sanitize_key`] rules and render it as an
/// S3 object key.
///
/// [`sanitize_key`] returns a `PathBuf` so `LocalDisk` can join it onto a root;
/// object keys are always `/`-separated, so the cleaned components are re-joined
/// here rather than formatted with the platform separator (which would produce
/// `a\b.txt` on Windows).
fn sanitized_key(key: &str) -> Result<String, StorageError> {
    let cleaned = sanitize_key(key)?;
    let object_key = cleaned
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if object_key.is_empty() {
        return Err(StorageError::InvalidKey(
            "key must not be empty after normalization".into(),
        ));
    }
    Ok(object_key)
}

/// Split an endpoint origin into `(scheme, authority)`.
fn parse_endpoint(endpoint: &str) -> Result<(String, String), StorageError> {
    let (scheme, rest) = if let Some(rest) = endpoint.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = endpoint.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(StorageError::Backend(format!(
            "endpoint must start with http:// or https:// (got `{endpoint}`)"
        )));
    };
    let authority = rest.split('/').next().unwrap_or(rest).trim_end_matches('/');
    if authority.is_empty() {
        return Err(StorageError::Backend(format!(
            "missing host in endpoint `{endpoint}`"
        )));
    }
    Ok((scheme.to_owned(), authority.to_owned()))
}

fn ensure_success(response: &S3Response, key: &str) -> Result<(), StorageError> {
    if response.status.is_success() {
        Ok(())
    } else {
        Err(backend_error(response, key))
    }
}

fn backend_error(response: &S3Response, key: &str) -> StorageError {
    let snippet: String = String::from_utf8_lossy(&response.body)
        .chars()
        .take(256)
        .collect();
    StorageError::Backend(format!(
        "S3 request for `{key}` failed: HTTP {} {snippet}",
        response.status
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(addressing: Addressing) -> S3Disk {
        let config = S3Config::new(
            "https://s3.amazonaws.com",
            "us-east-1",
            "examplebucket",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        )
        .with_addressing(addressing);
        S3Disk::new(config).expect("disk")
    }

    #[test]
    fn config_debug_redacts_secret() {
        let debug = format!("{:?}", disk(Addressing::VirtualHost));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("wJalrXUtnFEMI"));
    }

    #[test]
    fn resolve_target_path_vs_virtual_host() {
        let (host, uri) = disk(Addressing::Path).resolve_target("a/b.txt");
        assert_eq!(host, "s3.amazonaws.com");
        assert_eq!(uri, "/examplebucket/a/b.txt");

        let (host, uri) = disk(Addressing::VirtualHost).resolve_target("a/b.txt");
        assert_eq!(host, "examplebucket.s3.amazonaws.com");
        assert_eq!(uri, "/a/b.txt");
    }

    #[test]
    fn virtual_host_keeps_custom_port() {
        let config = S3Config::new(
            "http://127.0.0.1:9000",
            "us-east-1",
            "uploads",
            "id",
            "secret",
        )
        .with_addressing(Addressing::VirtualHost);
        let disk = S3Disk::new(config).expect("disk");
        let (host, _) = disk.resolve_target("k");
        assert_eq!(host, "uploads.127.0.0.1:9000");
    }

    #[test]
    fn presigned_get_url_matches_aws_vector() {
        // Virtual-host style reproduces the AWS documented presigned GET URL.
        let disk = disk(Addressing::VirtualHost);
        // 2013-05-24T00:00:00Z.
        let url = disk
            .presign(
                &Method::GET,
                "test.txt",
                Duration::from_hours(24),
                1_369_353_600,
            )
            .expect("presign");
        assert_eq!(
            url,
            "https://examplebucket.s3.amazonaws.com/test.txt?\
             X-Amz-Algorithm=AWS4-HMAC-SHA256&\
             X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&\
             X-Amz-Date=20130524T000000Z&\
             X-Amz-Expires=86400&\
             X-Amz-SignedHeaders=host&\
             X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn presigned_put_url_structure() {
        let disk = disk(Addressing::Path);
        let url = disk
            .presigned_put_url("covers/1.png", Duration::from_mins(15))
            .expect("presign");
        assert!(url.starts_with("https://s3.amazonaws.com/examplebucket/covers/1.png?"));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-Expires=900"));
        assert!(url.contains("X-Amz-SignedHeaders=host"));
        assert!(url.contains("&X-Amz-Signature="));
    }

    #[test]
    fn presign_clamps_expiry_to_seven_days() {
        let disk = disk(Addressing::Path);
        let url = disk
            .presign(&Method::GET, "k", Duration::from_hours(720), 1_369_353_600)
            .expect("presign");
        assert!(url.contains("X-Amz-Expires=604800"));
    }

    #[test]
    fn unsafe_keys_are_rejected_before_signing() {
        let disk = disk(Addressing::Path);
        assert!(matches!(
            disk.presigned_get_url("../secret", Duration::from_mins(1)),
            Err(StorageError::InvalidKey(_))
        ));
        assert!(matches!(
            disk.path_for("/etc/passwd"),
            Err(StorageError::InvalidKey(_))
        ));
    }

    #[test]
    fn path_for_is_unsupported_for_valid_keys() {
        let disk = disk(Addressing::Path);
        assert!(matches!(
            disk.path_for("avatars/user.png"),
            Err(StorageError::Unsupported(_))
        ));
    }

    #[test]
    fn parse_endpoint_variants() {
        assert_eq!(
            parse_endpoint("http://127.0.0.1:9000/").unwrap(),
            ("http".to_owned(), "127.0.0.1:9000".to_owned())
        );
        assert_eq!(
            parse_endpoint("https://oss-cn-hangzhou.aliyuncs.com").unwrap(),
            (
                "https".to_owned(),
                "oss-cn-hangzhou.aliyuncs.com".to_owned()
            )
        );
        assert!(parse_endpoint("127.0.0.1:9000").is_err());
        assert!(parse_endpoint("https://").is_err());
    }
}
