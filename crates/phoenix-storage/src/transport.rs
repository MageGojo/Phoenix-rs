//! Pluggable HTTP transport for the S3-compatible driver.
//!
//! [`S3Disk`](crate::S3Disk) talks to object storage through [`S3Http`], so
//! tests can point it at a local fake S3 server (plain HTTP on 127.0.0.1) or
//! swap in a transport that mangles requests. The default implementation,
//! [`HyperS3Http`], is a minimal one-request-per-connection hyper 1.x client
//! with rustls (system root certificates via `rustls-native-certs`).
//!
//! This mirrors the `PayHttp` seam in `phoenix-pay`; object uploads are
//! low-volume relative to a real CDN, so connection pooling is deliberately
//! out of scope.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::StorageError;

/// Boxed, `Send` future returned by the object-safe [`S3Http`] trait.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// One outgoing S3 request. Headers are already-signed `(name, value)` pairs;
/// the transport supplies `Host` and `Connection` itself.
#[derive(Clone, Debug)]
pub struct S3Request {
    /// HTTP method.
    pub method: Method,
    /// Absolute URL (`http://127.0.0.1:9000/bucket/key` or an `https://…`
    /// endpoint), including any presigned query string.
    pub url: String,
    /// Extra headers to send (`Authorization`, `x-amz-date`, …). Lowercase
    /// names are fine; HTTP header names are case-insensitive.
    pub headers: Vec<(String, String)>,
    /// Request body (empty for GET/DELETE/HEAD).
    pub body: Bytes,
}

/// One S3 response with the full body collected.
#[derive(Clone, Debug)]
pub struct S3Response {
    /// HTTP status.
    pub status: StatusCode,
    /// Response headers (`ETag`, `Content-Type`, …).
    pub headers: HeaderMap,
    /// Collected response body (may be empty).
    pub body: Bytes,
}

/// Async HTTP seam between [`S3Disk`](crate::S3Disk) and the object store.
///
/// Object-safe (returns a [`BoxFuture`]) so the driver can hold
/// `Arc<dyn S3Http>` and callers can inject fakes.
pub trait S3Http: Send + Sync {
    /// Perform one request and collect the full response.
    fn send(&self, request: S3Request) -> BoxFuture<Result<S3Response, StorageError>>;
}

/// Default [`S3Http`]: hyper 1.x + rustls with system root certificates.
#[derive(Clone, Debug, Default)]
pub struct HyperS3Http {
    _private: (),
}

impl HyperS3Http {
    /// Timeout applied to connect + request + body collection.
    const TIMEOUT: Duration = Duration::from_mins(1);

    /// New transport (stateless; TLS configuration is process-wide).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn tls_config() -> Result<Arc<ClientConfig>, StorageError> {
        static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
        CONFIG
            .get_or_init(|| {
                let loaded = rustls_native_certs::load_native_certs();
                let mut roots = RootCertStore::empty();
                let (added, _) = roots.add_parsable_certificates(loaded.certs);
                if added == 0 {
                    return Err(format!(
                        "no usable system root certificates ({} load errors)",
                        loaded.errors.len()
                    ));
                }
                Ok(Arc::new(
                    ClientConfig::builder()
                        .with_root_certificates(roots)
                        .with_no_client_auth(),
                ))
            })
            .clone()
            .map_err(StorageError::Backend)
    }

    async fn send_inner(request: S3Request) -> Result<S3Response, StorageError> {
        let target = UrlParts::parse(&request.url)?;
        let stream = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|error| StorageError::Backend(format!("connect {}: {error}", target.host)))?;
        if target.tls {
            let config = Self::tls_config()?;
            let server_name = ServerName::try_from(target.host.clone()).map_err(|_| {
                StorageError::Backend(format!("invalid TLS host `{}`", target.host))
            })?;
            let stream = TlsConnector::from(config)
                .connect(server_name, stream)
                .await
                .map_err(|error| StorageError::Backend(format!("TLS handshake: {error}")))?;
            Self::exchange(TokioIo::new(stream), &target, request).await
        } else {
            Self::exchange(TokioIo::new(stream), &target, request).await
        }
    }

    async fn exchange<T>(
        io: T,
        target: &UrlParts,
        request: S3Request,
    ) -> Result<S3Response, StorageError>
    where
        T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|error| StorageError::Backend(format!("HTTP handshake: {error}")))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let mut builder = hyper::Request::builder()
            .method(request.method)
            .uri(target.path_and_query.clone());
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let http_request = builder
            .header(header::HOST, &target.authority)
            .header(header::CONNECTION, HeaderValue::from_static("close"))
            .body(Full::new(request.body))
            .map_err(|error| StorageError::Backend(format!("build request: {error}")))?;

        let response = sender
            .send_request(http_request)
            .await
            .map_err(|error| StorageError::Backend(format!("send request: {error}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| StorageError::Backend(format!("read response body: {error}")))?
            .to_bytes();
        Ok(S3Response {
            status,
            headers,
            body,
        })
    }
}

impl S3Http for HyperS3Http {
    fn send(&self, request: S3Request) -> BoxFuture<Result<S3Response, StorageError>> {
        Box::pin(async move {
            tokio::time::timeout(Self::TIMEOUT, Self::send_inner(request))
                .await
                .map_err(|_| StorageError::Backend("S3 request timed out".to_owned()))?
        })
    }
}

/// Minimal absolute-URL splitter (scheme, host, port, path?query). S3 endpoint
/// URLs are plain `scheme://host[:port]/path[?query]` — no userinfo, no IPv6
/// literals.
struct UrlParts {
    tls: bool,
    host: String,
    port: u16,
    authority: String,
    path_and_query: String,
}

impl UrlParts {
    fn parse(url: &str) -> Result<Self, StorageError> {
        let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(StorageError::Backend(format!("unsupported S3 URL `{url}`")));
        };
        let (authority, path_and_query) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| StorageError::Backend(format!("invalid port in `{url}`")))?,
            ),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err(StorageError::Backend(format!("missing host in `{url}`")));
        }
        Ok(Self {
            tls,
            host: host.to_owned(),
            port,
            authority: authority.to_owned(),
            path_and_query: path_and_query.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::UrlParts;

    #[test]
    fn parses_s3_urls() {
        let parts =
            UrlParts::parse("https://examplebucket.s3.amazonaws.com/test.txt").expect("url");
        assert!(parts.tls);
        assert_eq!(parts.host, "examplebucket.s3.amazonaws.com");
        assert_eq!(parts.port, 443);
        assert_eq!(parts.path_and_query, "/test.txt");

        let parts =
            UrlParts::parse("http://127.0.0.1:9000/bucket/key?X-Amz-Signature=abc").expect("url");
        assert!(!parts.tls);
        assert_eq!(parts.port, 9000);
        assert_eq!(parts.authority, "127.0.0.1:9000");
        assert_eq!(parts.path_and_query, "/bucket/key?X-Amz-Signature=abc");

        assert!(UrlParts::parse("ftp://x").is_err());
        assert!(UrlParts::parse("https:///nohost").is_err());
        assert!(UrlParts::parse("http://h:70000/").is_err());
    }
}
