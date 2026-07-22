use std::{convert::Infallible, future::Future, pin::Pin, sync::Arc};

pub use bytes::Bytes;
pub use http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug)]
pub struct Request {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    params: Vec<(String, String)>,
    route_name: Option<String>,
    extensions: Extensions,
}

impl Request {
    #[must_use]
    pub fn new(method: Method, uri: Uri) -> Self {
        Self {
            method,
            uri,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            params: Vec::new(),
            route_name: None,
            extensions: Extensions::new(),
        }
    }

    #[must_use]
    pub fn from_parts(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            method,
            uri,
            headers,
            body,
            params: Vec::new(),
            route_name: None,
            extensions: Extensions::new(),
        }
    }

    #[must_use]
    pub fn method(&self) -> &Method {
        &self.method
    }

    #[must_use]
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Deserialize the request body as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when `Content-Type` is not JSON or the body is not
    /// valid JSON for `T`.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, JsonRejection> {
        let content_type = self
            .headers
            .get(header::CONTENT_TYPE)
            .ok_or(JsonRejection::MissingContentType)?
            .to_str()
            .map_err(|_| JsonRejection::UnsupportedContentType)?
            .parse::<mime::Mime>()
            .map_err(|_| JsonRejection::UnsupportedContentType)?;

        let is_json = content_type.type_() == mime::APPLICATION
            && (content_type.subtype() == mime::JSON || content_type.suffix() == Some(mime::JSON));
        if !is_json {
            return Err(JsonRejection::UnsupportedContentType);
        }

        serde_json::from_slice(&self.body).map_err(JsonRejection::InvalidJson)
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }

    #[must_use]
    pub fn route_name(&self) -> Option<&str> {
        self.route_name.as_deref()
    }

    #[must_use]
    pub fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }

    #[doc(hidden)]
    pub fn set_route(&mut self, name: Option<String>, params: Vec<(String, String)>) {
        self.route_name = name;
        self.params = params;
    }
}

#[derive(Debug, Error)]
pub enum JsonRejection {
    #[error("The Content-Type header must be application/json.")]
    MissingContentType,
    #[error("The Content-Type header must be application/json.")]
    UnsupportedContentType,
    #[error("The request body contains invalid JSON.")]
    InvalidJson(#[source] serde_json::Error),
}

impl JsonRejection {
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::MissingContentType | Self::UnsupportedContentType => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            Self::InvalidJson(_) => StatusCode::BAD_REQUEST,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Response {
    #[must_use]
    pub fn new(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: body.into(),
        }
    }

    #[must_use]
    pub fn text(body: impl Into<Bytes>) -> Self {
        let mut response = Self::new(StatusCode::OK, body);
        response.headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        response
    }

    fn json<T: Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        let mut response = Self::new(StatusCode::OK, serde_json::to_vec(value)?);
        response.headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Ok(response)
    }

    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    #[must_use]
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Append a validated header to the response.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeader`] when the name or value cannot be converted to
    /// an HTTP header.
    pub fn with_header(
        mut self,
        name: impl TryInto<HeaderName>,
        value: impl TryInto<HeaderValue>,
    ) -> Result<Self, InvalidHeader> {
        let name = name.try_into().map_err(|_| InvalidHeader)?;
        let value = value.try_into().map_err(|_| InvalidHeader)?;
        self.headers.insert(name, value);
        Ok(self)
    }

    #[doc(hidden)]
    pub fn into_parts(self) -> (StatusCode, HeaderMap, Bytes) {
        (self.status, self.headers, self.body)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InvalidHeader;

impl std::fmt::Display for InvalidHeader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid HTTP header name or value")
    }
}

impl std::error::Error for InvalidHeader {}

pub trait IntoResponse {
    fn into_response(self) -> Response;
}

#[derive(Clone, Debug)]
pub struct Json<T>(pub T);

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        Response::json(&self.0).unwrap_or_else(|_| {
            Response::text("Internal Server Error").with_status(StatusCode::INTERNAL_SERVER_ERROR)
        })
    }
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::text(self)
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response::text(self)
    }
}

impl IntoResponse for Bytes {
    fn into_response(self) -> Response {
        Response::new(StatusCode::OK, self)
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::new(StatusCode::NO_CONTENT, Bytes::new())
    }
}

impl<T> IntoResponse for (StatusCode, T)
where
    T: IntoResponse,
{
    fn into_response(self) -> Response {
        self.1.into_response().with_status(self.0)
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        }
    }
}

impl IntoResponse for Infallible {
    fn into_response(self) -> Response {
        match self {}
    }
}

impl IntoResponse for JsonRejection {
    fn into_response(self) -> Response {
        let status = self.status();
        Response::text(self.to_string()).with_status(status)
    }
}

pub trait Handler: Send + Sync + 'static {
    fn call(&self, request: Request) -> BoxFuture<Response>;
}

impl<F, Fut, Output> Handler for F
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Output> + Send + 'static,
    Output: IntoResponse + 'static,
{
    fn call(&self, request: Request) -> BoxFuture<Response> {
        let future = (self)(request);
        Box::pin(async move { future.await.into_response() })
    }
}

#[derive(Clone)]
pub struct Next {
    handler: Arc<dyn Handler>,
}

impl Next {
    #[must_use]
    fn new(handler: Arc<dyn Handler>) -> Self {
        Self { handler }
    }

    pub async fn run(self, request: Request) -> Response {
        self.handler.call(request).await
    }
}

pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response>;
}

pub struct MiddlewareFn<F>(F);

#[must_use]
pub fn middleware_fn<F>(function: F) -> MiddlewareFn<F> {
    MiddlewareFn(function)
}

impl<F, Fut, Output> Middleware for MiddlewareFn<F>
where
    F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Output> + Send + 'static,
    Output: IntoResponse + 'static,
{
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let future = (self.0)(request, next);
        Box::pin(async move { future.await.into_response() })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SecurityHeaders;

impl Middleware for SecurityHeaders {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let mut response = next.run(request).await;
            insert_default_header(
                response.headers_mut(),
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            );
            insert_default_header(
                response.headers_mut(),
                HeaderName::from_static("x-frame-options"),
                HeaderValue::from_static("DENY"),
            );
            insert_default_header(
                response.headers_mut(),
                header::REFERRER_POLICY,
                HeaderValue::from_static("strict-origin-when-cross-origin"),
            );
            response
        })
    }
}

fn insert_default_header(headers: &mut HeaderMap, name: HeaderName, value: HeaderValue) {
    headers.entry(name).or_insert(value);
}

struct MiddlewareHandler {
    middleware: Arc<dyn Middleware>,
    next: Arc<dyn Handler>,
}

impl Handler for MiddlewareHandler {
    fn call(&self, request: Request) -> BoxFuture<Response> {
        self.middleware
            .handle(request, Next::new(Arc::clone(&self.next)))
    }
}

#[must_use]
#[doc(hidden)]
pub fn apply_middleware(
    handler: Arc<dyn Handler>,
    middleware: &[Arc<dyn Middleware>],
) -> Arc<dyn Handler> {
    middleware.iter().rev().fold(handler, |next, current| {
        Arc::new(MiddlewareHandler {
            middleware: Arc::clone(current),
            next,
        })
    })
}
