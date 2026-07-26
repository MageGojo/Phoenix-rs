//! Pluggable HTTP transport for the real payment gateways.
//!
//! Providers talk to gateways through [`PayHttp`], so tests can point them at
//! a local fake gateway (plain HTTP on 127.0.0.1) or stub the transport
//! entirely. The default implementation, [`HyperPayHttp`], is a minimal
//! one-request-per-connection hyper 1.x client with rustls (system root
//! certificates via `rustls-native-certs`) — payment calls are low-volume, so
//! connection pooling is deliberately out of scope.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use phoenix_http::{BoxFuture, Bytes, HeaderMap, HeaderValue, Method, StatusCode, header};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::PayError;

/// One outgoing gateway request.
#[derive(Clone, Debug)]
pub struct GatewayRequest {
    /// HTTP method.
    pub method: Method,
    /// Absolute URL (`https://api.mch.weixin.qq.com/v3/...`).
    pub url: String,
    /// Extra headers (`Authorization`, `Content-Type`, ...).
    pub headers: Vec<(&'static str, String)>,
    /// Request body (empty for GET).
    pub body: Bytes,
}

/// One gateway response.
#[derive(Clone, Debug)]
pub struct GatewayResponse {
    /// HTTP status.
    pub status: StatusCode,
    /// Response headers (`Wechatpay-Signature`, ...).
    pub headers: HeaderMap,
    /// Collected response body.
    pub body: Bytes,
}

/// Async HTTP seam between providers and the outside world.
///
/// Object-safe like [`crate::PaymentProvider`] (returns
/// [`phoenix_http::BoxFuture`]) so providers can hold `Arc<dyn PayHttp>`.
pub trait PayHttp: Send + Sync {
    /// Perform one request and collect the full response.
    fn request(&self, request: GatewayRequest) -> BoxFuture<Result<GatewayResponse, PayError>>;
}

/// Default [`PayHttp`]: hyper 1.x + rustls with system root certificates.
#[derive(Clone, Debug, Default)]
pub struct HyperPayHttp {
    _private: (),
}

impl HyperPayHttp {
    /// Timeout applied to connect + request + body collection.
    const TIMEOUT: Duration = Duration::from_secs(30);

    /// New transport (stateless; TLS configuration is process-wide).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn tls_config() -> Result<Arc<ClientConfig>, PayError> {
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
            .map_err(PayError::Config)
    }

    async fn send(request: GatewayRequest) -> Result<GatewayResponse, PayError> {
        let target = UrlParts::parse(&request.url)?;
        let stream = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|error| PayError::Gateway(format!("connect {}: {error}", target.host)))?;
        if target.tls {
            let config = Self::tls_config()?;
            let server_name = ServerName::try_from(target.host.clone())
                .map_err(|_| PayError::Gateway(format!("invalid TLS host `{}`", target.host)))?;
            let stream = TlsConnector::from(config)
                .connect(server_name, stream)
                .await
                .map_err(|error| PayError::Gateway(format!("TLS handshake: {error}")))?;
            Self::exchange(TokioIo::new(stream), &target, request).await
        } else {
            Self::exchange(TokioIo::new(stream), &target, request).await
        }
    }

    async fn exchange<T>(
        io: T,
        target: &UrlParts,
        request: GatewayRequest,
    ) -> Result<GatewayResponse, PayError>
    where
        T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
    {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|error| PayError::Gateway(format!("HTTP handshake: {error}")))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let mut builder = hyper::Request::builder()
            .method(request.method)
            .uri(target.path_and_query.clone());
        for (name, value) in &request.headers {
            builder = builder.header(*name, value);
        }
        let http_request = builder
            .header(header::HOST, &target.authority)
            .header(header::CONNECTION, HeaderValue::from_static("close"))
            .body(http_body_util::Full::new(request.body))
            .map_err(|error| PayError::Gateway(format!("build request: {error}")))?;

        let response = sender
            .send_request(http_request)
            .await
            .map_err(|error| PayError::Gateway(format!("send request: {error}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| PayError::Gateway(format!("read response body: {error}")))?
            .to_bytes();
        Ok(GatewayResponse {
            status,
            headers,
            body,
        })
    }
}

impl PayHttp for HyperPayHttp {
    fn request(&self, request: GatewayRequest) -> BoxFuture<Result<GatewayResponse, PayError>> {
        Box::pin(async move {
            tokio::time::timeout(Self::TIMEOUT, Self::send(request))
                .await
                .map_err(|_| PayError::Gateway("gateway request timed out".to_owned()))?
        })
    }
}

/// Minimal absolute-URL splitter (scheme, host, port, path?query). Payment
/// gateway URLs are plain `https://host[:port]/path` — no userinfo, no IPv6
/// literals.
struct UrlParts {
    tls: bool,
    host: String,
    port: u16,
    authority: String,
    path_and_query: String,
}

impl UrlParts {
    fn parse(url: &str) -> Result<Self, PayError> {
        let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(PayError::Gateway(format!(
                "unsupported gateway URL `{url}`"
            )));
        };
        let (authority, path_and_query) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| PayError::Gateway(format!("invalid port in `{url}`")))?,
            ),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err(PayError::Gateway(format!("missing host in `{url}`")));
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

/// Path + query of an absolute URL, the canonical form `WeChat` signs over.
pub(crate) fn path_and_query(url: &str) -> Result<String, PayError> {
    UrlParts::parse(url).map(|parts| parts.path_and_query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gateway_urls() {
        let parts = UrlParts::parse("https://api.mch.weixin.qq.com/v3/pay?x=1").expect("url");
        assert!(parts.tls);
        assert_eq!(parts.host, "api.mch.weixin.qq.com");
        assert_eq!(parts.port, 443);
        assert_eq!(parts.path_and_query, "/v3/pay?x=1");

        let parts = UrlParts::parse("http://127.0.0.1:8080").expect("url");
        assert!(!parts.tls);
        assert_eq!(parts.port, 8080);
        assert_eq!(parts.authority, "127.0.0.1:8080");
        assert_eq!(parts.path_and_query, "/");

        assert!(UrlParts::parse("ftp://x").is_err());
        assert!(UrlParts::parse("https:///nohost").is_err());
        assert!(UrlParts::parse("http://h:70000/").is_err());
        assert_eq!(
            path_and_query("https://openapi.alipay.com/gateway.do").expect("path"),
            "/gateway.do"
        );
    }
}
