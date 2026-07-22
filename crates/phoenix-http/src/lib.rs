use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Arc,
};

pub use bytes::Bytes;
use futures_util::{Stream, StreamExt};
pub use http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
pub use mime::Mime;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Transport scheme established by the socket or a trusted proxy layer.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TransportScheme {
    #[default]
    Http,
    Https,
}

impl TransportScheme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    #[must_use]
    pub const fn is_secure(self) -> bool {
        matches!(self, Self::Https)
    }
}

/// Connection metadata populated before Phoenix middleware executes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionInfo {
    peer_addr: Option<SocketAddr>,
    scheme: TransportScheme,
    alpn_protocol: Option<String>,
}

/// A validated request-scoped Content Security Policy nonce.
#[derive(Clone, Eq, PartialEq)]
pub struct CspNonce(String);

impl CspNonce {
    /// Validate a CSP base64-value-like token before it enters an HTML attribute or Header.
    ///
    /// # Errors
    ///
    /// Returns an error for short, excessively long, or non-token values.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidCspNonce> {
        let value = value.into();
        if !(16..=128).contains(&value.len())
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'_' | b'-' | b'=')
            })
        {
            return Err(InvalidCspNonce);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CspNonce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("CspNonce")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("CSP nonce is invalid")]
pub struct InvalidCspNonce;

impl ConnectionInfo {
    #[must_use]
    pub const fn new(
        peer_addr: Option<SocketAddr>,
        scheme: TransportScheme,
        alpn_protocol: Option<String>,
    ) -> Self {
        Self {
            peer_addr,
            scheme,
            alpn_protocol,
        }
    }

    #[must_use]
    pub const fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }

    #[must_use]
    pub const fn scheme(&self) -> TransportScheme {
        self.scheme
    }

    #[must_use]
    pub fn alpn_protocol(&self) -> Option<&str> {
        self.alpn_protocol.as_deref()
    }

    #[must_use]
    pub const fn is_secure(&self) -> bool {
        self.scheme.is_secure()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RouteManifest(Arc<HashMap<String, String>>);

impl RouteManifest {
    #[must_use]
    pub fn new(routes: Arc<HashMap<String, String>>) -> Self {
        Self(routes)
    }

    #[must_use]
    pub fn routes(&self) -> &HashMap<String, String> {
        &self.0
    }
}

/// Request metadata retained while a handler converts its output into a response.
///
/// This keeps request-aware response features such as CSP nonces and named-route
/// manifests available even when the handler consumes the [`Request`]. Sensitive
/// request Header values are never retained.
#[derive(Clone)]
pub struct ResponseContext {
    uri: Uri,
    page_request: bool,
    csp_nonce: Option<CspNonce>,
    route_manifest: Option<RouteManifest>,
}

impl ResponseContext {
    #[must_use]
    pub fn from_request(request: &Request) -> Self {
        Self {
            uri: request.uri().clone(),
            page_request: request
                .headers()
                .get("x-phoenix-page")
                .is_some_and(|value| value == "1"),
            csp_nonce: request.extensions().get::<CspNonce>().cloned(),
            route_manifest: request.extensions().get::<RouteManifest>().cloned(),
        }
    }

    #[must_use]
    pub const fn uri(&self) -> &Uri {
        &self.uri
    }

    #[must_use]
    pub const fn is_page_request(&self) -> bool {
        self.page_request
    }

    #[must_use]
    pub const fn csp_nonce(&self) -> Option<&CspNonce> {
        self.csp_nonce.as_ref()
    }

    #[must_use]
    pub const fn route_manifest(&self) -> Option<&RouteManifest> {
        self.route_manifest.as_ref()
    }
}

impl std::fmt::Debug for ResponseContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseContext")
            .field("path", &self.uri.path())
            .field("page_request", &self.page_request)
            .field("has_csp_nonce", &self.csp_nonce.is_some())
            .field("has_route_manifest", &self.route_manifest.is_some())
            .finish_non_exhaustive()
    }
}

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

pub trait FromRequest: Sized + Send + 'static {
    type Rejection: IntoResponse + Send + 'static;

    /// Extract a strongly typed value from an already buffered request.
    ///
    /// # Errors
    ///
    /// Returns a typed rejection when the request cannot be converted into
    /// the requested value.
    fn from_request(request: &Request) -> Result<Self, Self::Rejection>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query<T>(pub T);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path<T>(pub T);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header<T>(pub T);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form<T>(pub T);

/// A clone of application state stored in the request extension map.
#[derive(Clone, Debug)]
pub struct State<T>(pub T);

macro_rules! extractor_deref {
    ($extractor:ident) => {
        impl<T> Deref for $extractor<T> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<T> DerefMut for $extractor<T> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

extractor_deref!(Query);
extractor_deref!(Path);
extractor_deref!(Header);
extractor_deref!(Form);
extractor_deref!(Json);
extractor_deref!(State);

#[derive(Clone, Copy, Debug, Error)]
#[error("Required application state is unavailable.")]
pub struct StateRejection;

impl<T> FromRequest for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = StateRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        request
            .extensions()
            .get::<T>()
            .cloned()
            .map(Self)
            .ok_or(StateRejection)
    }
}

#[derive(Debug, Error)]
#[error("The query string is invalid.")]
pub struct QueryRejection(#[source] serde_urlencoded::de::Error);

#[derive(Debug, Error)]
#[error("The route parameters are invalid.")]
pub struct PathRejection(#[source] StringMapRejection);

#[derive(Debug, Error)]
pub enum HeaderRejection {
    #[error("A request header contains invalid text.")]
    InvalidValue,
    #[error("The request headers are invalid.")]
    InvalidData(#[source] StringMapRejection),
}

#[derive(Debug, Error)]
pub enum StringMapRejection {
    #[error("could not encode string fields")]
    Encode(#[source] serde_urlencoded::ser::Error),
    #[error("could not decode string fields")]
    Decode(#[source] serde_urlencoded::de::Error),
}

#[derive(Debug, Error)]
pub enum FormRejection {
    #[error("The Content-Type header must be application/x-www-form-urlencoded.")]
    MissingContentType,
    #[error("The Content-Type header must be application/x-www-form-urlencoded.")]
    UnsupportedContentType,
    #[error("The form body is invalid.")]
    InvalidForm(#[source] serde_urlencoded::de::Error),
}

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = QueryRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        serde_urlencoded::from_str(request.uri().query().unwrap_or_default())
            .map(Self)
            .map_err(QueryRejection)
    }
}

impl<T> FromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = PathRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        deserialize_string_pairs(request.params.iter().cloned())
            .map(Self)
            .map_err(PathRejection)
    }
}

impl<T> FromRequest for Header<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = HeaderRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let pairs = request
            .headers()
            .iter()
            .map(|(name, header)| {
                header
                    .to_str()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
                    .map_err(|_| HeaderRejection::InvalidValue)
            })
            .collect::<Result<Vec<_>, _>>()?;
        deserialize_string_pairs(pairs)
            .map(Self)
            .map_err(HeaderRejection::InvalidData)
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = JsonRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        request.json().map(Self)
    }
}

impl<T> FromRequest for Form<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = FormRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .ok_or(FormRejection::MissingContentType)?
            .to_str()
            .map_err(|_| FormRejection::UnsupportedContentType)?
            .parse::<mime::Mime>()
            .map_err(|_| FormRejection::UnsupportedContentType)?;
        if content_type.type_() != mime::APPLICATION
            || content_type.subtype().as_str() != "x-www-form-urlencoded"
        {
            return Err(FormRejection::UnsupportedContentType);
        }
        serde_urlencoded::from_bytes(request.body())
            .map(Self)
            .map_err(FormRejection::InvalidForm)
    }
}

fn deserialize_string_pairs<T>(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> Result<T, StringMapRejection>
where
    T: DeserializeOwned,
{
    let fields: HashMap<String, String> = pairs.into_iter().collect();
    let encoded = serde_urlencoded::to_string(fields).map_err(StringMapRejection::Encode)?;
    serde_urlencoded::from_str(&encoded).map_err(StringMapRejection::Decode)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Multipart<T = MultipartData>(pub T);

impl<T> Deref for Multipart<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Multipart<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait FromMultipart: Sized + Send + 'static {
    /// Convert parsed multipart fields into an application DTO.
    ///
    /// # Errors
    ///
    /// Returns a stable multipart rejection when required fields are missing
    /// or cannot be converted.
    fn from_multipart(multipart: MultipartData) -> Result<Self, MultipartRejection>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultipartData {
    fields: Vec<MultipartField>,
}

impl MultipartData {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&MultipartField> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn fields(&self, name: &str) -> impl Iterator<Item = &MultipartField> {
        self.fields.iter().filter(move |field| field.name == name)
    }

    #[must_use]
    pub fn all(&self) -> &[MultipartField] {
        &self.fields
    }

    pub fn remove(&mut self, name: &str) -> Option<MultipartField> {
        let index = self.fields.iter().position(|field| field.name == name)?;
        Some(self.fields.remove(index))
    }

    #[must_use]
    pub fn into_fields(self) -> Vec<MultipartField> {
        self.fields
    }
}

impl FromMultipart for MultipartData {
    fn from_multipart(multipart: Self) -> Result<Self, MultipartRejection> {
        Ok(multipart)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultipartField {
    name: String,
    file_name: Option<String>,
    content_type: Option<String>,
    data: Bytes,
}

impl MultipartField {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    #[must_use]
    pub fn bytes(&self) -> &Bytes {
        &self.data
    }

    /// Read a non-file field as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns an error when the field contains non-UTF-8 bytes.
    pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.data)
    }
}

#[derive(Debug, Error)]
pub enum MultipartRejection {
    #[error("The Content-Type header must be multipart/form-data.")]
    MissingContentType,
    #[error("The Content-Type header must be multipart/form-data with a boundary.")]
    UnsupportedContentType,
    #[error("The multipart body is invalid.")]
    InvalidBody,
    #[error("The multipart field {0} is invalid or missing.")]
    InvalidField(String),
}

impl<T> FromRequest for Multipart<T>
where
    T: FromMultipart,
{
    type Rejection = MultipartRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .ok_or(MultipartRejection::MissingContentType)?
            .to_str()
            .map_err(|_| MultipartRejection::UnsupportedContentType)?
            .parse::<mime::Mime>()
            .map_err(|_| MultipartRejection::UnsupportedContentType)?;
        if content_type.type_() != mime::MULTIPART || content_type.subtype().as_str() != "form-data"
        {
            return Err(MultipartRejection::UnsupportedContentType);
        }
        let boundary = content_type
            .get_param("boundary")
            .map(|name| name.as_str().to_owned())
            .filter(|boundary| !boundary.is_empty() && boundary.len() <= 70)
            .ok_or(MultipartRejection::UnsupportedContentType)?;
        let fields = parse_multipart(request.body(), &boundary)?;
        T::from_multipart(MultipartData { fields }).map(Self)
    }
}

fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<MultipartField>, MultipartRejection> {
    let delimiter = format!("--{boundary}").into_bytes();
    if !body.starts_with(&delimiter) {
        return Err(MultipartRejection::InvalidBody);
    }

    let mut fields = Vec::new();
    let mut cursor = delimiter.len();
    loop {
        if body.get(cursor..cursor + 2) == Some(b"--") {
            cursor += 2;
            if body
                .get(cursor..)
                .is_some_and(|tail| tail.is_empty() || tail == b"\r\n")
            {
                return Ok(fields);
            }
            return Err(MultipartRejection::InvalidBody);
        }
        if body.get(cursor..cursor + 2) != Some(b"\r\n") {
            return Err(MultipartRejection::InvalidBody);
        }
        cursor += 2;

        let header_end = find_bytes(&body[cursor..], b"\r\n\r\n")
            .map(|index| cursor + index)
            .ok_or(MultipartRejection::InvalidBody)?;
        let headers = std::str::from_utf8(&body[cursor..header_end])
            .map_err(|_| MultipartRejection::InvalidBody)?;
        let data_start = header_end + 4;
        let mut marker = b"\r\n".to_vec();
        marker.extend_from_slice(&delimiter);
        let data_end = find_bytes(&body[data_start..], &marker)
            .map(|index| data_start + index)
            .ok_or(MultipartRejection::InvalidBody)?;

        let (name, file_name, content_type) = parse_multipart_headers(headers)?;
        fields.push(MultipartField {
            name,
            file_name,
            content_type,
            data: Bytes::copy_from_slice(&body[data_start..data_end]),
        });
        cursor = data_end + marker.len();
    }
}

fn parse_multipart_headers(
    headers: &str,
) -> Result<(String, Option<String>, Option<String>), MultipartRejection> {
    let mut disposition = None;
    let mut content_type = None;
    for line in headers.split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or(MultipartRejection::InvalidBody)?;
        match name.trim().to_ascii_lowercase().as_str() {
            "content-disposition" => disposition = Some(value.trim()),
            "content-type" => {
                value
                    .trim()
                    .parse::<mime::Mime>()
                    .map_err(|_| MultipartRejection::InvalidBody)?;
                content_type = Some(value.trim().to_owned());
            }
            _ => {}
        }
    }

    let disposition = disposition.ok_or(MultipartRejection::InvalidBody)?;
    let parts = split_quoted(disposition, ';')?;
    if parts.first().map(|part| part.trim()) != Some("form-data") {
        return Err(MultipartRejection::InvalidBody);
    }
    let mut name = None;
    let mut file_name = None;
    for part in parts.iter().skip(1) {
        let (key, value) = part
            .split_once('=')
            .ok_or(MultipartRejection::InvalidBody)?;
        let value = unquote_parameter(value.trim())?;
        match key.trim() {
            "name" => name = Some(value),
            "filename" => file_name = Some(value),
            _ => {}
        }
    }
    Ok((
        name.filter(|value| !value.is_empty())
            .ok_or(MultipartRejection::InvalidBody)?,
        file_name,
        content_type,
    ))
}

fn split_quoted(value: &str, separator: char) -> Result<Vec<&str>, MultipartRejection> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == separator && !quoted {
            parts.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }
    if quoted || escaped {
        return Err(MultipartRejection::InvalidBody);
    }
    parts.push(&value[start..]);
    Ok(parts)
}

fn unquote_parameter(value: &str) -> Result<String, MultipartRejection> {
    if let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        let mut output = String::with_capacity(value.len());
        let mut escaped = false;
        for character in value.chars() {
            if escaped {
                if character != '"' && character != '\\' {
                    return Err(MultipartRejection::InvalidBody);
                }
                output.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                output.push(character);
            }
        }
        if escaped {
            return Err(MultipartRejection::InvalidBody);
        }
        Ok(output)
    } else if value.bytes().all(|byte| byte.is_ascii_graphic()) {
        Ok(value.to_owned())
    } else {
        Err(MultipartRejection::InvalidBody)
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send + 'static>>;

pub enum ResponseBody {
    Buffered(Bytes),
    Stream(ByteStream),
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buffered(bytes) => formatter
                .debug_tuple("Buffered")
                .field(&bytes.len())
                .finish(),
            Self::Stream(_) => formatter.write_str("Stream(<body>)"),
        }
    }
}

#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: ResponseBody,
}

impl Response {
    #[must_use]
    pub fn new(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: ResponseBody::Buffered(body.into()),
        }
    }

    #[must_use]
    pub fn stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Bytes> + Send + 'static,
    {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: ResponseBody::Stream(Box::pin(stream.map(Ok))),
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

    /// Return a buffered response body.
    ///
    /// # Panics
    ///
    /// Panics when called for a streaming response. Use [`Self::is_streaming`]
    /// before inspecting responses that may stream.
    #[must_use]
    pub fn body(&self) -> &Bytes {
        match &self.body {
            ResponseBody::Buffered(body) => body,
            ResponseBody::Stream(_) => panic!("streaming response bodies are not buffered"),
        }
    }

    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        matches!(self.body, ResponseBody::Stream(_))
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
    pub fn into_parts(self) -> (StatusCode, HeaderMap, ResponseBody) {
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

/// A validated HTTP redirect response.
#[derive(Clone, Debug)]
pub struct Redirect {
    status: StatusCode,
    location: HeaderValue,
}

impl Redirect {
    /// Create a `302 Found` redirect.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeader`] when the location contains invalid header bytes.
    pub fn to(location: impl TryInto<HeaderValue>) -> Result<Self, InvalidHeader> {
        Self::with_status(StatusCode::FOUND, location)
    }

    /// Create a `303 See Other` redirect, suitable after a form submission.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeader`] when the location contains invalid header bytes.
    pub fn see_other(location: impl TryInto<HeaderValue>) -> Result<Self, InvalidHeader> {
        Self::with_status(StatusCode::SEE_OTHER, location)
    }

    /// Create a `307 Temporary Redirect` response.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeader`] when the location contains invalid header bytes.
    pub fn temporary(location: impl TryInto<HeaderValue>) -> Result<Self, InvalidHeader> {
        Self::with_status(StatusCode::TEMPORARY_REDIRECT, location)
    }

    fn with_status(
        status: StatusCode,
        location: impl TryInto<HeaderValue>,
    ) -> Result<Self, InvalidHeader> {
        Ok(Self {
            status,
            location: location.try_into().map_err(|_| InvalidHeader)?,
        })
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        let mut response = Response::new(self.status, Bytes::new());
        response
            .headers_mut()
            .insert(header::LOCATION, self.location);
        response
    }
}

/// A buffered binary response with safe download headers.
#[derive(Clone, Debug)]
pub struct Download {
    body: Bytes,
    content_type: Mime,
    disposition: &'static str,
    file_name: String,
    cache_control: HeaderValue,
}

impl Download {
    #[must_use]
    pub fn attachment(
        body: impl Into<Bytes>,
        file_name: impl Into<String>,
        content_type: Mime,
    ) -> Self {
        Self::new(body, file_name, content_type, "attachment")
    }

    #[must_use]
    pub fn inline(
        body: impl Into<Bytes>,
        file_name: impl Into<String>,
        content_type: Mime,
    ) -> Self {
        Self::new(body, file_name, content_type, "inline")
    }

    fn new(
        body: impl Into<Bytes>,
        file_name: impl Into<String>,
        content_type: Mime,
        disposition: &'static str,
    ) -> Self {
        Self {
            body: body.into(),
            content_type,
            disposition,
            file_name: file_name.into(),
            cache_control: HeaderValue::from_static("private, no-store"),
        }
    }

    /// Override the conservative private download cache policy.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeader`] when the value contains invalid header bytes.
    pub fn with_cache_control(
        mut self,
        value: impl TryInto<HeaderValue>,
    ) -> Result<Self, InvalidHeader> {
        self.cache_control = value.try_into().map_err(|_| InvalidHeader)?;
        Ok(self)
    }
}

impl IntoResponse for Download {
    fn into_response(self) -> Response {
        let fallback = ascii_file_name(&self.file_name);
        let encoded = utf8_percent_encode(&self.file_name, NON_ALPHANUMERIC);
        let content_disposition = format!(
            "{}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}",
            self.disposition
        );
        let mut response = Response::new(StatusCode::OK, self.body);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(self.content_type.as_ref())
                .expect("a parsed MIME value is a valid header"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&content_disposition)
                .expect("sanitized content disposition is a valid header"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, self.cache_control);
        response.headers_mut().insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        response
    }
}

fn ascii_file_name(file_name: &str) -> String {
    let sanitized = file_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.trim_matches(['.', '_', '-']).is_empty() {
        "download".to_owned()
    } else {
        sanitized
    }
}

pub trait IntoResponse {
    fn into_response(self) -> Response;

    /// Convert a handler result while retaining safe request-local response metadata.
    #[doc(hidden)]
    fn into_response_with_context(self, _context: &ResponseContext) -> Response
    where
        Self: Sized,
    {
        self.into_response()
    }
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

    fn into_response_with_context(self, context: &ResponseContext) -> Response {
        self.1
            .into_response_with_context(context)
            .with_status(self.0)
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

    fn into_response_with_context(self, context: &ResponseContext) -> Response {
        match self {
            Ok(value) => value.into_response_with_context(context),
            Err(error) => error.into_response_with_context(context),
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
        rejection_response(status, &self.to_string())
    }
}

impl IntoResponse for QueryRejection {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::BAD_REQUEST, &self.to_string())
    }
}

impl IntoResponse for PathRejection {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::BAD_REQUEST, &self.to_string())
    }
}

impl IntoResponse for HeaderRejection {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::BAD_REQUEST, &self.to_string())
    }
}

impl IntoResponse for FormRejection {
    fn into_response(self) -> Response {
        let status = match self {
            Self::MissingContentType | Self::UnsupportedContentType => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            Self::InvalidForm(_) => StatusCode::BAD_REQUEST,
        };
        rejection_response(status, &self.to_string())
    }
}

impl IntoResponse for StateRejection {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::INTERNAL_SERVER_ERROR, &self.to_string())
    }
}

impl IntoResponse for MultipartRejection {
    fn into_response(self) -> Response {
        let status = match self {
            Self::MissingContentType | Self::UnsupportedContentType => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            Self::InvalidBody | Self::InvalidField(_) => StatusCode::BAD_REQUEST,
        };
        rejection_response(status, &self.to_string())
    }
}

fn rejection_response(status: StatusCode, message: &str) -> Response {
    #[derive(Serialize)]
    struct RejectionBody<'a> {
        message: &'a str,
    }

    Json(RejectionBody { message })
        .into_response()
        .with_status(status)
}

pub trait Handler: Send + Sync + 'static {
    fn call(&self, request: Request) -> BoxFuture<Response>;
}

pub struct TypedHandler {
    handler: Arc<dyn Handler>,
}

impl Handler for TypedHandler {
    fn call(&self, request: Request) -> BoxFuture<Response> {
        self.handler.call(request)
    }
}

pub trait IntoTypedHandler<Args>: Send + Sync + Sized + 'static {
    fn into_typed_handler(self) -> TypedHandler;
}

#[must_use]
pub fn typed<H, Args>(handler: H) -> TypedHandler
where
    H: IntoTypedHandler<Args>,
{
    handler.into_typed_handler()
}

impl<F, Fut, Output, A> IntoTypedHandler<(A,)> for F
where
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Output> + Send + 'static,
    Output: IntoResponse + 'static,
    A: FromRequest,
{
    fn into_typed_handler(self) -> TypedHandler {
        let function = Arc::new(self);
        let handler = move |request: Request| {
            let function = Arc::clone(&function);
            async move {
                let context = ResponseContext::from_request(&request);
                match A::from_request(&request) {
                    Ok(first) => function(first).await.into_response_with_context(&context),
                    Err(rejection) => rejection.into_response_with_context(&context),
                }
            }
        };
        TypedHandler {
            handler: Arc::new(handler),
        }
    }
}

macro_rules! impl_typed_handler {
    ($(($type:ident, $value:ident)),+ $(,)?) => {
        impl<F, Fut, Output, $($type),+> IntoTypedHandler<($($type,)+)> for F
        where
            F: Fn($($type),+) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Output> + Send + 'static,
            Output: IntoResponse + 'static,
            $($type: FromRequest,)+
        {
            fn into_typed_handler(self) -> TypedHandler {
                let function = Arc::new(self);
                let handler = move |request: Request| {
                    let function = Arc::clone(&function);
                    async move {
                        let context = ResponseContext::from_request(&request);
                        $(
                            let $value = match $type::from_request(&request) {
                                Ok(value) => value,
                                Err(rejection) => {
                                    return rejection.into_response_with_context(&context);
                                }
                            };
                        )+
                        function($($value),+).await.into_response_with_context(&context)
                    }
                };
                TypedHandler {
                    handler: Arc::new(handler),
                }
            }
        }
    };
}

impl_typed_handler!((A, first), (B, second));
impl_typed_handler!((A, first), (B, second), (C, third));
impl_typed_handler!((A, first), (B, second), (C, third), (D, fourth));

impl<F, Fut, Output> Handler for F
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Output> + Send + 'static,
    Output: IntoResponse + 'static,
{
    fn call(&self, request: Request) -> BoxFuture<Response> {
        let context = ResponseContext::from_request(&request);
        let future = (self)(request);
        Box::pin(async move { future.await.into_response_with_context(&context) })
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

/// Insert cloneable application state into every request handled downstream.
#[derive(Clone, Debug)]
pub struct StateMiddleware<T> {
    value: T,
}

impl<T> StateMiddleware<T> {
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self { value }
    }
}

impl<T> Middleware for StateMiddleware<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        request.extensions_mut().insert(self.value.clone());
        Box::pin(async move { next.run(request).await })
    }
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
        let context = ResponseContext::from_request(&request);
        let future = (self.0)(request, next);
        Box::pin(async move { future.await.into_response_with_context(&context) })
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

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct SearchInput {
        page: u32,
        term: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct UserPath {
        user: u64,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct RequestHeaders {
        #[serde(rename = "x-request-id")]
        request_id: String,
    }

    struct UploadInput {
        title: String,
        document: MultipartField,
    }

    #[test]
    fn csp_nonce_validation_and_debug_are_fail_closed() {
        let nonce = CspNonce::new("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(nonce.as_str(), "0123456789abcdef0123456789abcdef");
        let debug = format!("{nonce:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(nonce.as_str()));
        for invalid in [
            "short",
            "0123456789abcde!",
            "0123456789abcdef\r\n",
            &"a".repeat(129),
        ] {
            assert!(CspNonce::new(invalid).is_err());
        }

        let mut request = Request::new(Method::GET, "/account?reset_token=secret".parse().unwrap());
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, HeaderValue::from_static("session=secret"));
        request
            .headers_mut()
            .insert("x-phoenix-page", HeaderValue::from_static("1"));
        let context = ResponseContext::from_request(&request);
        let debug = format!("{context:?}");
        assert!(context.is_page_request());
        assert!(debug.contains("/account"));
        assert!(!debug.contains("reset_token"));
        assert!(!debug.contains("secret"));
    }

    impl FromMultipart for UploadInput {
        fn from_multipart(mut multipart: MultipartData) -> Result<Self, MultipartRejection> {
            let title = multipart
                .remove("title")
                .ok_or_else(|| MultipartRejection::InvalidField("title".to_owned()))?
                .text()
                .map_err(|_| MultipartRejection::InvalidField("title".to_owned()))?
                .to_owned();
            let document = multipart
                .remove("document")
                .ok_or_else(|| MultipartRejection::InvalidField("document".to_owned()))?;
            Ok(Self { title, document })
        }
    }

    #[test]
    fn extracts_query_path_headers_json_and_form() {
        let query_request = Request::new(
            Method::GET,
            "/search?page=2&term=rust%20web".parse().expect("valid URI"),
        );
        assert_eq!(
            Query::<SearchInput>::from_request(&query_request)
                .expect("query should extract")
                .0,
            SearchInput {
                page: 2,
                term: "rust web".to_owned(),
            }
        );

        let mut path_request = Request::new(Method::GET, "/users/42".parse().expect("valid URI"));
        path_request.set_route(None, vec![("user".to_owned(), "42".to_owned())]);
        assert_eq!(
            Path::<UserPath>::from_request(&path_request)
                .expect("path should extract")
                .0,
            UserPath { user: 42 }
        );

        let mut header_request = Request::new(Method::GET, "/".parse().expect("valid URI"));
        header_request
            .headers_mut()
            .insert("x-request-id", HeaderValue::from_static("request-123"));
        assert_eq!(
            Header::<RequestHeaders>::from_request(&header_request)
                .expect("headers should extract")
                .0,
            RequestHeaders {
                request_id: "request-123".to_owned(),
            }
        );

        let mut json_headers = HeaderMap::new();
        json_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let json_request = Request::from_parts(
            Method::POST,
            "/".parse().expect("valid URI"),
            json_headers,
            Bytes::from_static(br#"{"page":3,"term":"contracts"}"#),
        );
        assert_eq!(
            Json::<SearchInput>::from_request(&json_request)
                .expect("JSON should extract")
                .0,
            SearchInput {
                page: 3,
                term: "contracts".to_owned(),
            }
        );

        let mut form_headers = HeaderMap::new();
        form_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let form_request = Request::from_parts(
            Method::POST,
            "/".parse().expect("valid URI"),
            form_headers,
            Bytes::from_static(b"page=4&term=typed+form"),
        );
        assert_eq!(
            Form::<SearchInput>::from_request(&form_request)
                .expect("form should extract")
                .0,
            SearchInput {
                page: 4,
                term: "typed form".to_owned(),
            }
        );
    }

    #[test]
    fn extracts_multipart_text_and_file_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data; boundary=phoenix-boundary"),
        );
        let body = concat!(
            "--phoenix-boundary\r\n",
            "Content-Disposition: form-data; name=\"title\"\r\n\r\n",
            "Typed upload\r\n",
            "--phoenix-boundary\r\n",
            "Content-Disposition: form-data; name=\"document\"; filename=\"notes.txt\"\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "hello\r\n",
            "--phoenix-boundary--\r\n",
        );
        let request = Request::from_parts(
            Method::POST,
            "/upload".parse().expect("valid URI"),
            headers,
            Bytes::from_static(body.as_bytes()),
        );

        let multipart =
            Multipart::<MultipartData>::from_request(&request).expect("multipart should extract");
        assert_eq!(
            multipart
                .field("title")
                .expect("title")
                .text()
                .expect("UTF-8"),
            "Typed upload"
        );
        let file = multipart.field("document").expect("document");
        assert_eq!(file.file_name(), Some("notes.txt"));
        assert_eq!(file.content_type(), Some("text/plain"));
        assert_eq!(file.bytes(), &Bytes::from_static(b"hello"));

        let input = Multipart::<UploadInput>::from_request(&request)
            .expect("typed multipart DTO should extract")
            .0;
        assert_eq!(input.title, "Typed upload");
        assert_eq!(input.document.file_name(), Some("notes.txt"));
    }

    #[tokio::test]
    async fn typed_handler_extracts_arguments_and_maps_rejections() {
        let handler = typed(|Query(input): Query<SearchInput>| async move { Json(input) });
        let response = handler
            .call(Request::new(
                Method::GET,
                "/search?page=5&term=handler".parse().expect("valid URI"),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<SearchInput>(response.body()).expect("JSON response"),
            SearchInput {
                page: 5,
                term: "handler".to_owned(),
            }
        );

        let response = handler
            .call(Request::new(
                Method::GET,
                "/search?page=wrong&term=handler"
                    .parse()
                    .expect("valid URI"),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn state_middleware_feeds_typed_handlers_and_missing_state_is_safe() {
        #[derive(Clone, Debug)]
        struct AppName(&'static str);

        let handler = typed(|State(name): State<AppName>| async move { name.0 });
        let request = Request::new(Method::GET, "/".parse().expect("valid URI"));
        let response = Handler::call(&handler, request).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let handler: Arc<dyn Handler> = Arc::new(handler);
        let handler = apply_middleware(
            handler,
            &[Arc::new(StateMiddleware::new(AppName("Phoenix")))],
        );
        let response = handler
            .call(Request::new(Method::GET, "/".parse().expect("valid URI")))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "Phoenix");
    }

    #[tokio::test]
    async fn handler_response_conversion_retains_safe_request_context() {
        struct ContextResponse;

        impl IntoResponse for ContextResponse {
            fn into_response(self) -> Response {
                Response::text("missing context")
            }

            fn into_response_with_context(self, context: &ResponseContext) -> Response {
                let nonce = context.csp_nonce().map_or("missing", CspNonce::as_str);
                let route_count = context
                    .route_manifest()
                    .map_or(0, |manifest| manifest.routes().len());
                Response::text(format!("{}|{nonce}|{route_count}", context.uri()))
            }
        }

        let handler =
            |_request: Request| async { Ok::<_, Response>((StatusCode::CREATED, ContextResponse)) };
        let mut request = Request::new(Method::GET, "/context".parse().unwrap());
        request
            .extensions_mut()
            .insert(CspNonce::new("0123456789abcdef0123456789abcdef").unwrap());
        request
            .extensions_mut()
            .insert(RouteManifest::new(Arc::new(HashMap::from([(
                "home".to_owned(),
                "/".to_owned(),
            )]))));

        let response = Handler::call(&handler, request).await;

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.body(),
            "/context|0123456789abcdef0123456789abcdef|1"
        );
    }

    #[test]
    fn redirect_and_download_headers_reject_injection_and_preserve_unicode() {
        assert!(Redirect::see_other("/safe\r\nx-injected: yes").is_err());
        let redirect = Redirect::see_other("/account")
            .expect("valid redirect")
            .into_response();
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(redirect.headers()[header::LOCATION], "/account");

        let response = Download::attachment(
            Bytes::from_static(b"profile"),
            "证书\r\nset-cookie: bad.mobileconfig",
            "application/x-apple-aspen-config"
                .parse()
                .expect("valid MIME"),
        )
        .into_response();
        let disposition = response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .expect("ASCII header");
        assert!(disposition.contains("filename*=UTF-8''"));
        assert!(!disposition.contains("\r\n"));
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }
}
