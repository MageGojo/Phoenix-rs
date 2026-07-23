use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, header};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::{TestApp, TestResponse, cookie::CookieJar};

const PAGE_REQUEST_HEADER: &str = "x-phoenix-page";

/// Fluent builder for a single HTTP request against a [`TestApp`].
pub struct RequestBuilder {
    address: std::net::SocketAddr,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
    cookies: Arc<Mutex<CookieJar>>,
}

impl RequestBuilder {
    pub(crate) fn new(app: &TestApp, method: Method, path: String) -> Self {
        let path = if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        };
        Self {
            address: app.address(),
            method,
            path,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            cookies: app.cookies(),
        }
    }

    /// Replace the request body.
    #[must_use]
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Set a request header, replacing any previous value for the same name.
    ///
    /// # Panics
    ///
    /// Panics when `name` or `value` is not a valid HTTP header.
    #[must_use]
    pub fn header(
        mut self,
        name: impl TryInto<HeaderName, Error = impl std::fmt::Debug>,
        value: impl TryInto<HeaderValue, Error = impl std::fmt::Debug>,
    ) -> Self {
        let name = name.try_into().expect("valid header name");
        let value = value.try_into().expect("valid header value");
        self.headers.insert(name, value);
        self
    }

    /// Mark this request as a Phoenix page-protocol navigation (`X-Phoenix-Page: 1`).
    #[must_use]
    pub fn page_protocol(self) -> Self {
        self.header(PAGE_REQUEST_HEADER, "1")
    }

    /// Send the request and return a [`TestResponse`].
    ///
    /// # Panics
    ///
    /// Panics on transport, protocol, or body read failures so test failures
    /// surface as assertion-style panics.
    pub async fn send(self) -> TestResponse {
        self.try_send()
            .await
            .expect("TestApp request should complete")
    }

    async fn try_send(mut self) -> Result<TestResponse, Box<dyn std::error::Error + Send + Sync>> {
        {
            let jar = self
                .cookies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cookie) = jar.header_value() {
                self.headers.entry(header::COOKIE).or_insert(cookie);
            }
        }

        let stream = TcpStream::connect(self.address).await?;
        let io = TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let host = self.address.to_string();
        let mut request = hyper::Request::builder()
            .method(self.method)
            .uri(self.path)
            .body(Full::new(self.body))?;
        *request.headers_mut() = self.headers;
        request
            .headers_mut()
            .entry(header::HOST)
            .or_insert(HeaderValue::from_str(&host)?);
        request
            .headers_mut()
            .entry(header::CONNECTION)
            .or_insert(HeaderValue::from_static("close"));

        let response = sender.send_request(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = collect_body(response.into_body()).await?;

        {
            let mut jar = self
                .cookies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            jar.store_from_response(&headers);
        }

        Ok(TestResponse::new(status, headers, body))
    }
}

async fn collect_body(body: Incoming) -> Result<Bytes, hyper::Error> {
    let collected = body.collect().await?;
    Ok(collected.to_bytes())
}
