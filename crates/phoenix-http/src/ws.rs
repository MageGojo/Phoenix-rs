//! Controlled HTTP/1.1 WebSocket upgrade facade.
//!
//! HTTP/2 extended CONNECT (RFC 8441) is intentionally unsupported.

use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version, header};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        self,
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig, frame::coding::CloseCode as TungsteniteCloseCode},
    },
};

use crate::{FromRequest, IntoResponse, Request, Response, rejection_response};

const DEFAULT_MAX_MESSAGE_SIZE: usize = 64 * 1024;
const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024;
const MAX_CONFIGURED_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Takeable Hyper upgrade handle installed by the HTTP/1 connection layer.
#[derive(Clone, Debug)]
pub struct ConnectionUpgrade {
    on_upgrade: Arc<Mutex<Option<OnUpgrade>>>,
}

impl ConnectionUpgrade {
    /// Wrap a Hyper [`OnUpgrade`] so handlers can take it through `&Request`.
    #[must_use]
    pub fn new(on_upgrade: OnUpgrade) -> Self {
        Self {
            on_upgrade: Arc::new(Mutex::new(Some(on_upgrade))),
        }
    }

    /// Take the pending upgrade exactly once.
    #[must_use]
    pub fn take(&self) -> Option<OnUpgrade> {
        self.on_upgrade.lock().ok()?.take()
    }
}

/// RFC 6455 close status codes used by the Phoenix WebSocket facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseCode(u16);

impl CloseCode {
    pub const NORMAL: Self = Self(1000);
    pub const AWAY: Self = Self(1001);
    pub const PROTOCOL: Self = Self(1002);
    pub const UNSUPPORTED: Self = Self(1003);
    pub const ABNORMAL: Self = Self(1006);
    pub const INVALID_PAYLOAD: Self = Self(1007);
    pub const POLICY: Self = Self(1008);
    pub const MESSAGE_TOO_BIG: Self = Self(1009);
    pub const MANDATORY_EXT: Self = Self(1010);
    pub const INTERNAL: Self = Self(1011);

    #[must_use]
    pub const fn new(code: u16) -> Self {
        Self(code)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl From<CloseCode> for TungsteniteCloseCode {
    fn from(value: CloseCode) -> Self {
        Self::from(value.0)
    }
}

impl From<TungsteniteCloseCode> for CloseCode {
    fn from(value: TungsteniteCloseCode) -> Self {
        Self(u16::from(value))
    }
}

/// A WebSocket close frame observed by application code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseFrame {
    pub code: CloseCode,
    pub reason: String,
}

/// A WebSocket message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
    Close(Option<CloseFrame>),
}

impl Message {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    #[must_use]
    pub fn binary(data: impl Into<Bytes>) -> Self {
        Self::Binary(data.into())
    }

    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    #[must_use]
    pub const fn is_binary(&self) -> bool {
        matches!(self, Self::Binary(_))
    }

    #[must_use]
    pub const fn is_close(&self) -> bool {
        matches!(self, Self::Close(_))
    }

    /// Return text payload when this is a text message.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketError::Protocol`] for non-text messages.
    pub fn into_text(self) -> Result<String, WebSocketError> {
        match self {
            Self::Text(text) => Ok(text),
            _ => Err(WebSocketError::Protocol),
        }
    }

    fn into_tungstenite(self) -> tungstenite::Message {
        match self {
            Self::Text(text) => tungstenite::Message::Text(text.into()),
            Self::Binary(data) => tungstenite::Message::Binary(data),
            Self::Ping(data) => tungstenite::Message::Ping(data),
            Self::Pong(data) => tungstenite::Message::Pong(data),
            Self::Close(frame) => {
                tungstenite::Message::Close(frame.map(|frame| tungstenite::protocol::CloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.into(),
                }))
            }
        }
    }

    fn from_tungstenite(message: tungstenite::Message) -> Option<Self> {
        match message {
            tungstenite::Message::Text(text) => Some(Self::Text(text.to_string())),
            tungstenite::Message::Binary(data) => Some(Self::Binary(data)),
            tungstenite::Message::Ping(data) => Some(Self::Ping(data)),
            tungstenite::Message::Pong(data) => Some(Self::Pong(data)),
            tungstenite::Message::Close(frame) => {
                Some(Self::Close(frame.map(|frame| CloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.to_string(),
                })))
            }
            tungstenite::Message::Frame(_) => None,
        }
    }
}

/// Application-facing WebSocket I/O error. Details stay redacted.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WebSocketError {
    #[error("The WebSocket connection is closed.")]
    Closed,
    #[error("The WebSocket protocol was violated.")]
    Protocol,
    #[error("The WebSocket message exceeded the configured size limit.")]
    MessageTooLarge,
    #[error("The WebSocket transport failed.")]
    Transport,
}

impl WebSocketError {
    fn from_tungstenite(error: &tungstenite::Error) -> Self {
        match error {
            tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
                Self::Closed
            }
            tungstenite::Error::Capacity(_) => Self::MessageTooLarge,
            tungstenite::Error::Protocol(_) | tungstenite::Error::Utf8(_) => Self::Protocol,
            _ => Self::Transport,
        }
    }
}

/// An upgraded WebSocket connection.
pub struct WebSocket {
    inner: WebSocketStream<TokioIo<Upgraded>>,
}

impl std::fmt::Debug for WebSocket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WebSocket")
    }
}

impl WebSocket {
    /// Receive the next message. `None` means the peer closed cleanly.
    pub async fn recv(&mut self) -> Option<Result<Message, WebSocketError>> {
        loop {
            match self.inner.next().await? {
                Ok(message) => {
                    if let Some(message) = Message::from_tungstenite(message) {
                        return Some(Ok(message));
                    }
                }
                Err(error) => return Some(Err(WebSocketError::from_tungstenite(&error))),
            }
        }
    }

    /// Send a text or binary message (or control frame).
    ///
    /// # Errors
    ///
    /// Returns a redacted transport or protocol error when the send fails.
    pub async fn send(&mut self, message: Message) -> Result<(), WebSocketError> {
        self.inner
            .send(message.into_tungstenite())
            .await
            .map_err(|error| WebSocketError::from_tungstenite(&error))
    }

    /// Send a close frame and flush. Prefer this over dropping for graceful shutdown.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport error when the close cannot be written.
    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), WebSocketError> {
        self.send(Message::Close(frame)).await
    }
}

#[derive(Clone, Debug)]
enum OriginPolicy {
    Allowlist(Vec<String>),
    Any,
}

/// Extractor that validates an HTTP/1.1 WebSocket handshake and upgrades the connection.
pub struct WebSocketUpgrade {
    sec_websocket_key: HeaderValue,
    on_upgrade: OnUpgrade,
    origin: Option<String>,
    origin_policy: OriginPolicy,
    max_message_size: usize,
    max_frame_size: usize,
}

impl std::fmt::Debug for WebSocketUpgrade {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebSocketUpgrade")
            .field("origin_policy", &self.origin_policy)
            .field("max_message_size", &self.max_message_size)
            .field("max_frame_size", &self.max_frame_size)
            .field("has_origin", &self.origin.is_some())
            .finish_non_exhaustive()
    }
}

impl WebSocketUpgrade {
    /// Allow an exact Origin value. Repeating builds an allowlist.
    #[must_use]
    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        match &mut self.origin_policy {
            OriginPolicy::Allowlist(origins) => origins.push(origin.into()),
            OriginPolicy::Any => {
                self.origin_policy = OriginPolicy::Allowlist(vec![origin.into()]);
            }
        }
        self
    }

    /// Disable Origin checks. Intended for local tests, not production browsers.
    #[must_use]
    pub fn any_origin(mut self) -> Self {
        self.origin_policy = OriginPolicy::Any;
        self
    }

    /// Cap reassembled incoming message size.
    ///
    /// # Errors
    ///
    /// Rejects zero and values above 16 MiB.
    pub fn max_message_size(mut self, bytes: usize) -> Result<Self, WebSocketConfigError> {
        if bytes == 0 || bytes > MAX_CONFIGURED_MESSAGE_SIZE {
            return Err(WebSocketConfigError::InvalidMessageSize);
        }
        self.max_message_size = bytes;
        Ok(self)
    }

    /// Cap a single incoming frame payload.
    ///
    /// # Errors
    ///
    /// Rejects zero and values above 16 MiB.
    pub fn max_frame_size(mut self, bytes: usize) -> Result<Self, WebSocketConfigError> {
        if bytes == 0 || bytes > MAX_CONFIGURED_MESSAGE_SIZE {
            return Err(WebSocketConfigError::InvalidFrameSize);
        }
        self.max_frame_size = bytes;
        Ok(self)
    }

    /// Complete the handshake and run `callback` on the upgraded socket.
    ///
    /// Origin is checked here so [`Self::allowed_origin`] / [`Self::any_origin`] can run first.
    ///
    /// # Panics
    ///
    /// Panics only if the derived `Sec-WebSocket-Accept` value is somehow not a
    /// valid header value (should be unreachable for RFC 6455 accept keys).
    pub fn on_upgrade<F, Fut>(self, callback: F) -> Response
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Err(rejection) = self.check_origin() {
            return rejection.into_response();
        }

        let Self {
            sec_websocket_key,
            on_upgrade,
            max_message_size,
            max_frame_size,
            ..
        } = self;

        let accept = derive_accept_key(sec_websocket_key.as_bytes());
        let config = WebSocketConfig::default()
            .max_message_size(Some(max_message_size))
            .max_frame_size(Some(max_frame_size))
            .write_buffer_size(0);

        tokio::spawn(async move {
            let Ok(upgraded) = on_upgrade.await else {
                return;
            };
            let socket = WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                Some(config),
            )
            .await;
            callback(WebSocket { inner: socket }).await;
        });

        let mut response = Response::new(StatusCode::SWITCHING_PROTOCOLS, Bytes::new());
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        response
            .headers_mut()
            .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        response.headers_mut().insert(
            HeaderName::from_static("sec-websocket-accept"),
            HeaderValue::from_str(&accept).expect("derived accept key is a valid header"),
        );
        response
    }

    fn check_origin(&self) -> Result<(), WebSocketUpgradeRejection> {
        match &self.origin_policy {
            OriginPolicy::Any => Ok(()),
            OriginPolicy::Allowlist(origins) => {
                let Some(origin) = self.origin.as_deref() else {
                    return Err(WebSocketUpgradeRejection::OriginDenied);
                };
                if origins.iter().any(|allowed| allowed == origin) {
                    Ok(())
                } else {
                    Err(WebSocketUpgradeRejection::OriginDenied)
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WebSocketConfigError {
    #[error("The WebSocket message size limit must be between 1 byte and 16 MiB.")]
    InvalidMessageSize,
    #[error("The WebSocket frame size limit must be between 1 byte and 16 MiB.")]
    InvalidFrameSize,
}

/// Why a WebSocket upgrade extractor rejected the request.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WebSocketUpgradeRejection {
    #[error("WebSocket upgrades require the GET method.")]
    MethodNotGet,
    #[error("WebSocket upgrades require HTTP/1.1.")]
    UnsupportedVersion,
    #[error("The WebSocket upgrade headers are invalid.")]
    InvalidHandshake,
    #[error("The WebSocket Origin is not allowed.")]
    OriginDenied,
    #[error("A WebSocket upgrade is unavailable on this connection.")]
    UpgradeUnavailable,
}

impl WebSocketUpgradeRejection {
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::OriginDenied => StatusCode::FORBIDDEN,
            Self::UnsupportedVersion => StatusCode::HTTP_VERSION_NOT_SUPPORTED,
            Self::MethodNotGet | Self::InvalidHandshake | Self::UpgradeUnavailable => {
                StatusCode::BAD_REQUEST
            }
        }
    }
}

impl IntoResponse for WebSocketUpgradeRejection {
    fn into_response(self) -> Response {
        rejection_response(self.status(), &self.to_string())
    }
}

impl IntoResponse for WebSocketConfigError {
    fn into_response(self) -> Response {
        rejection_response(StatusCode::INTERNAL_SERVER_ERROR, &self.to_string())
    }
}

impl FromRequest for WebSocketUpgrade {
    type Rejection = WebSocketUpgradeRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        if *request.method() != Method::GET {
            return Err(WebSocketUpgradeRejection::MethodNotGet);
        }
        if request.version() != Version::HTTP_11 {
            return Err(WebSocketUpgradeRejection::UnsupportedVersion);
        }
        if !header_contains_token(request.headers(), header::CONNECTION, "upgrade")
            || !header_eq_ignore_ascii_case(request.headers(), header::UPGRADE, "websocket")
        {
            return Err(WebSocketUpgradeRejection::InvalidHandshake);
        }
        let version = request
            .headers()
            .get(HeaderName::from_static("sec-websocket-version"))
            .ok_or(WebSocketUpgradeRejection::InvalidHandshake)?;
        if version.as_bytes() != b"13" {
            return Err(WebSocketUpgradeRejection::InvalidHandshake);
        }
        let sec_websocket_key = request
            .headers()
            .get(HeaderName::from_static("sec-websocket-key"))
            .cloned()
            .ok_or(WebSocketUpgradeRejection::InvalidHandshake)?;
        if !is_plausible_websocket_key(sec_websocket_key.as_bytes()) {
            return Err(WebSocketUpgradeRejection::InvalidHandshake);
        }

        let origin = request
            .headers()
            .get(header::ORIGIN)
            .map(|value| {
                value
                    .to_str()
                    .map(str::to_owned)
                    .map_err(|_| WebSocketUpgradeRejection::InvalidHandshake)
            })
            .transpose()?;

        let on_upgrade = request
            .extensions()
            .get::<ConnectionUpgrade>()
            .and_then(ConnectionUpgrade::take)
            .ok_or(WebSocketUpgradeRejection::UpgradeUnavailable)?;

        Ok(Self {
            sec_websocket_key,
            on_upgrade,
            origin,
            origin_policy: OriginPolicy::Allowlist(Vec::new()),
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        })
    }
}

fn header_contains_token(headers: &HeaderMap, name: HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|text| {
            text.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
    })
}

fn header_eq_ignore_ascii_case(headers: &HeaderMap, name: HeaderName, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn is_plausible_websocket_key(key: &[u8]) -> bool {
    // RFC 6455: 16 bytes of random data, base64-encoded => 24 ASCII chars.
    (16..=128).contains(&key.len())
        && key
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Extensions, Method, Request, Version};

    fn pending_upgrade() -> OnUpgrade {
        hyper::upgrade::on(http::Request::new(()))
    }

    fn upgrade_request(origin: Option<&str>) -> Request {
        let mut request = Request::from_server_parts(
            Method::GET,
            "/ws".parse().unwrap(),
            Version::HTTP_11,
            HeaderMap::new(),
            Extensions::new(),
            Bytes::new(),
        );
        request
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        request
            .headers_mut()
            .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        request.headers_mut().insert(
            HeaderName::from_static("sec-websocket-version"),
            HeaderValue::from_static("13"),
        );
        request.headers_mut().insert(
            HeaderName::from_static("sec-websocket-key"),
            HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        request
            .extensions_mut()
            .insert(ConnectionUpgrade::new(pending_upgrade()));
        request
    }

    #[test]
    fn default_origin_policy_denies_until_allowlisted_or_relaxed() {
        let request = upgrade_request(Some("https://app.example"));
        let upgrade = WebSocketUpgrade::from_request(&request).unwrap();
        assert!(matches!(
            upgrade.check_origin(),
            Err(WebSocketUpgradeRejection::OriginDenied)
        ));

        let request = upgrade_request(Some("https://app.example"));
        let upgrade = WebSocketUpgrade::from_request(&request)
            .unwrap()
            .allowed_origin("https://app.example");
        assert!(upgrade.check_origin().is_ok());

        let request = upgrade_request(None);
        let upgrade = WebSocketUpgrade::from_request(&request)
            .unwrap()
            .any_origin();
        assert!(upgrade.check_origin().is_ok());
    }

    #[tokio::test]
    async fn any_origin_upgrade_returns_switching_protocols() {
        let request = upgrade_request(None);
        let response = WebSocketUpgrade::from_request(&request)
            .unwrap()
            .any_origin()
            .on_upgrade(|_| async {});
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    }

    #[test]
    fn denied_origin_returns_forbidden_instead_of_switching_protocols() {
        let request = upgrade_request(Some("https://evil.example"));
        let response = WebSocketUpgrade::from_request(&request)
            .unwrap()
            .allowed_origin("https://app.example")
            .on_upgrade(|_| async {});
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn accept_key_matches_rfc_sample() {
        assert_eq!(
            derive_accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn rejects_http2_and_bad_handshake_headers() {
        let request = upgrade_request(Some("https://app.example"));
        let mut http2 = Request::from_server_parts(
            Method::GET,
            "/ws".parse().unwrap(),
            Version::HTTP_2,
            request.headers().clone(),
            Extensions::new(),
            Bytes::new(),
        );
        http2
            .extensions_mut()
            .insert(ConnectionUpgrade::new(pending_upgrade()));
        assert!(matches!(
            WebSocketUpgrade::from_request(&http2),
            Err(WebSocketUpgradeRejection::UnsupportedVersion)
        ));

        let mut bad = upgrade_request(Some("https://app.example"));
        bad.headers_mut()
            .insert(header::UPGRADE, HeaderValue::from_static("not-websocket"));
        assert!(matches!(
            WebSocketUpgrade::from_request(&bad),
            Err(WebSocketUpgradeRejection::InvalidHandshake)
        ));
    }

    #[test]
    fn missing_upgrade_extension_is_unavailable() {
        let mut request = upgrade_request(None);
        *request.extensions_mut() = Extensions::new();
        assert!(matches!(
            WebSocketUpgrade::from_request(&request),
            Err(WebSocketUpgradeRejection::UpgradeUnavailable)
        ));
    }
}
