//! Security middleware with production-safe defaults for Phoenix applications.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use phoenix_http::{
    BoxFuture, Bytes, HeaderMap, HeaderName, HeaderValue, Method, Middleware, Next, Request,
    Response, StatusCode, header,
};
use serde_json::Value;

/// A request identifier available to controllers and logs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestIdValue(pub String);

/// Adds a fresh unpredictable request ID to every request and response.
#[derive(Clone, Copy, Debug, Default)]
pub struct RequestId;

impl Middleware for RequestId {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let id = random_token(16);
            request.extensions_mut().insert(RequestIdValue(id.clone()));
            let mut response = next.run(request).await;
            if let Ok(value) = HeaderValue::from_str(&id) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static("x-request-id"), value);
            }
            response
        })
    }
}

/// The client address after applying the trusted proxy policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClientIp(pub IpAddr);

/// Resolves forwarding headers only when the direct peer is explicitly trusted.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies {
    trusted: Arc<HashSet<IpAddr>>,
}

impl TrustedProxies {
    #[must_use]
    pub fn new(proxies: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            trusted: Arc::new(proxies.into_iter().collect()),
        }
    }
}

impl Middleware for TrustedProxies {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let trusted = Arc::clone(&self.trusted);
        Box::pin(async move {
            if let Some(peer) = request.extensions().get::<SocketAddr>().copied() {
                let client = if trusted.contains(&peer.ip()) {
                    forwarded_client(request.headers(), peer.ip(), &trusted)
                } else {
                    peer.ip()
                };
                request.extensions_mut().insert(ClientIp(client));
            }
            next.run(request).await
        })
    }
}

fn forwarded_client(headers: &HeaderMap, peer: IpAddr, trusted: &HashSet<IpAddr>) -> IpAddr {
    let Some(value) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    else {
        return peer;
    };
    let mut client = peer;
    for hop in value
        .split(',')
        .rev()
        .filter_map(|part| part.trim().parse::<IpAddr>().ok())
    {
        if !trusted.contains(&client) {
            break;
        }
        client = hop;
    }
    client
}

/// Rejects requests whose HTTP Host is not explicitly allowed.
#[derive(Clone, Debug)]
pub struct HostAllowlist {
    allowed: Arc<HashSet<String>>,
}

impl HostAllowlist {
    #[must_use]
    pub fn new(hosts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: Arc::new(
                hosts
                    .into_iter()
                    .map(Into::into)
                    .map(|host: String| host.to_ascii_lowercase())
                    .collect(),
            ),
        }
    }
}

impl Middleware for HostAllowlist {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let allowed = Arc::clone(&self.allowed);
        Box::pin(async move {
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .and_then(normalize_host);
            if !host.is_some_and(|host| allowed.contains(&host)) {
                return Response::text("Invalid Host").with_status(StatusCode::BAD_REQUEST);
            }
            next.run(request).await
        })
    }
}

fn normalize_host(host: &str) -> Option<String> {
    let authority = host.parse::<http::uri::Authority>().ok()?;
    Some(authority.host().trim_end_matches('.').to_ascii_lowercase())
}

/// Cross-origin policy for browser requests.
#[derive(Clone, Debug)]
pub struct CorsConfig {
    pub allowed_origins: HashSet<String>,
    pub allowed_methods: HashSet<Method>,
    pub allowed_headers: HashSet<HeaderName>,
    pub allow_credentials: bool,
    pub max_age: Duration,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: HashSet::new(),
            allowed_methods: [Method::GET, Method::HEAD, Method::POST]
                .into_iter()
                .collect(),
            allowed_headers: [header::CONTENT_TYPE].into_iter().collect(),
            allow_credentials: false,
            max_age: Duration::from_mins(10),
        }
    }
}

/// Applies an explicit CORS allowlist and handles preflight requests.
#[derive(Clone, Debug)]
pub struct Cors {
    config: Arc<CorsConfig>,
}

impl Cors {
    #[must_use]
    pub fn new(config: CorsConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl Middleware for Cors {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let config = Arc::clone(&self.config);
        Box::pin(async move {
            let origin = request
                .headers()
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned);
            let Some(origin) = origin else {
                return next.run(request).await;
            };
            if !config.allowed_origins.contains(&origin) {
                return Response::text("Origin not allowed").with_status(StatusCode::FORBIDDEN);
            }
            let preflight = request.method() == Method::OPTIONS
                && request
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
            if !preflight && !config.allowed_methods.contains(request.method()) {
                return Response::text("Cross-origin method not allowed")
                    .with_status(StatusCode::FORBIDDEN);
            }
            let mut response = if preflight {
                if !valid_preflight(request.headers(), &config) {
                    return Response::text("CORS preflight rejected")
                        .with_status(StatusCode::FORBIDDEN);
                }
                Response::new(StatusCode::NO_CONTENT, Bytes::new())
            } else {
                next.run(request).await
            };
            apply_cors_headers(response.headers_mut(), &origin, &config, preflight);
            response
        })
    }
}

fn valid_preflight(headers: &HeaderMap, config: &CorsConfig) -> bool {
    let method = headers
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Method>().ok());
    if !method.is_some_and(|method| config.allowed_methods.contains(&method)) {
        return false;
    }
    headers
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|headers| {
            headers.split(',').all(|name| {
                name.trim()
                    .parse::<HeaderName>()
                    .is_ok_and(|name| config.allowed_headers.contains(&name))
            })
        })
}

fn apply_cors_headers(headers: &mut HeaderMap, origin: &str, config: &CorsConfig, preflight: bool) {
    if let Ok(origin) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if config.allow_credentials {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
    if preflight {
        let methods = join_header(config.allowed_methods.iter().map(Method::as_str));
        let allowed_headers = join_header(config.allowed_headers.iter().map(HeaderName::as_str));
        if let Ok(value) = HeaderValue::from_str(&methods) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, value);
        }
        if let Ok(value) = HeaderValue::from_str(&allowed_headers) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        if let Ok(value) = HeaderValue::from_str(&config.max_age.as_secs().to_string()) {
            headers.insert(header::ACCESS_CONTROL_MAX_AGE, value);
        }
    }
}

fn join_header<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    values.join(", ")
}

/// Fixed-window rate limit configuration.
#[derive(Clone, Copy, Debug)]
pub struct RateLimitConfig {
    pub requests: u64,
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests: 60,
            window: Duration::from_mins(1),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RateWindow {
    started: Instant,
    count: u64,
}

/// Limits requests by resolved client IP.
#[derive(Clone, Debug)]
pub struct RateLimit {
    config: RateLimitConfig,
    clients: Arc<Mutex<HashMap<IpAddr, RateWindow>>>,
}

impl RateLimit {
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Middleware for RateLimit {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let config = self.config;
        let clients = Arc::clone(&self.clients);
        Box::pin(async move {
            let client = request
                .extensions()
                .get::<ClientIp>()
                .map(|client| client.0)
                .or_else(|| request.extensions().get::<SocketAddr>().map(SocketAddr::ip))
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            let limited = {
                let mut clients = clients
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                let window = clients.entry(client).or_insert(RateWindow {
                    started: now,
                    count: 0,
                });
                if now.duration_since(window.started) >= config.window {
                    *window = RateWindow {
                        started: now,
                        count: 0,
                    };
                }
                window.count = window.count.saturating_add(1);
                (window.count > config.requests).then(|| {
                    config
                        .window
                        .saturating_sub(now.duration_since(window.started))
                })
            };
            if let Some(retry_after) = limited {
                let mut response =
                    Response::text("Too Many Requests").with_status(StatusCode::TOO_MANY_REQUESTS);
                if let Ok(value) = HeaderValue::from_str(&retry_after.as_secs().max(1).to_string())
                {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                return response;
            }
            next.run(request).await
        })
    }
}

#[derive(Clone, Debug)]
struct SessionRecord {
    values: HashMap<String, Value>,
    expires_at: Instant,
}

/// In-process server-side session store.
#[derive(Clone, Debug)]
pub struct SessionStore {
    records: Arc<Mutex<HashMap<String, SessionRecord>>>,
    ttl: Duration,
}

impl SessionStore {
    #[must_use]
    pub fn memory(ttl: Duration) -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    fn open(&self, id: Option<&str>) -> (Session, bool) {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        records.retain(|_, record| record.expires_at > now);
        let existing = id.filter(|id| records.contains_key(*id));
        let id = existing.map_or_else(|| random_token(32), ToOwned::to_owned);
        records
            .entry(id.clone())
            .and_modify(|record| record.expires_at = now + self.ttl)
            .or_insert_with(|| SessionRecord {
                values: HashMap::new(),
                expires_at: now + self.ttl,
            });
        (
            Session {
                id: Arc::new(Mutex::new(id)),
                records: Arc::clone(&self.records),
                ttl: self.ttl,
            },
            existing.is_none(),
        )
    }
}

/// A cloneable handle to one server-side session.
#[derive(Clone, Debug)]
pub struct Session {
    id: Arc<Mutex<String>>,
    records: Arc<Mutex<HashMap<String, SessionRecord>>>,
    ttl: Duration,
}

impl Session {
    #[must_use]
    pub fn id(&self) -> String {
        self.id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<Value> {
        self.with_record(|record| record.values.get(key).cloned())
            .flatten()
    }

    pub fn put(&self, key: impl Into<String>, value: impl Into<Value>) {
        self.with_record_mut(|record| {
            record.values.insert(key.into(), value.into());
        });
    }

    pub fn remove(&self, key: &str) {
        self.with_record_mut(|record| {
            record.values.remove(key);
        });
    }

    /// Replace the public session ID while preserving server-side values.
    pub fn regenerate(&self) {
        self.rotate(false);
    }

    /// Clear all server-side values and replace the public session ID.
    pub fn invalidate(&self) {
        self.rotate(true);
    }

    fn rotate(&self, clear: bool) {
        let mut id = self
            .id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_id = std::mem::take(&mut *id);
        let mut record = records.remove(&old_id).unwrap_or_else(|| SessionRecord {
            values: HashMap::new(),
            expires_at: Instant::now() + self.ttl,
        });
        if clear {
            record.values.clear();
        }
        record.expires_at = Instant::now() + self.ttl;
        *id = random_token(32);
        records.insert(id.clone(), record);
    }

    fn with_record<T>(&self, operation: impl FnOnce(&SessionRecord) -> T) -> Option<T> {
        let id = self.id();
        let records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        records.get(&id).map(operation)
    }

    fn with_record_mut<T>(&self, operation: impl FnOnce(&mut SessionRecord) -> T) -> Option<T> {
        let id = self.id();
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        records.get_mut(&id).map(operation)
    }
}

/// Cookie settings for server-side sessions.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub cookie_name: String,
    pub path: String,
    pub domain: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    pub max_age: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "phoenix_session".to_owned(),
            path: "/".to_owned(),
            domain: None,
            secure: true,
            http_only: true,
            same_site: SameSite::Lax,
            max_age: Duration::from_hours(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// Loads and persists server-side sessions through a secure ID cookie.
#[derive(Clone, Debug)]
pub struct SessionMiddleware {
    store: SessionStore,
    config: Arc<SessionConfig>,
}

impl SessionMiddleware {
    #[must_use]
    pub fn new(store: SessionStore, config: SessionConfig) -> Self {
        Self {
            store,
            config: Arc::new(config),
        }
    }
}

impl Middleware for SessionMiddleware {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let store = self.store.clone();
        let config = Arc::clone(&self.config);
        Box::pin(async move {
            let id = cookie_value(request.headers(), &config.cookie_name);
            let (session, _created) = store.open(id.as_deref());
            request.extensions_mut().insert(session.clone());
            let mut response = next.run(request).await;
            if let Ok(cookie) = HeaderValue::from_str(&session_cookie(&session, &config)) {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        })
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

fn session_cookie(session: &Session, config: &SessionConfig) -> String {
    let mut cookie = format!(
        "{}={}; Path={}; Max-Age={}; SameSite={}",
        config.cookie_name,
        session.id(),
        config.path,
        config.max_age.as_secs(),
        config.same_site.as_str()
    );
    if let Some(domain) = &config.domain {
        cookie.push_str("; Domain=");
        cookie.push_str(domain);
    }
    if config.secure || config.same_site == SameSite::None {
        cookie.push_str("; Secure");
    }
    if config.http_only {
        cookie.push_str("; HttpOnly");
    }
    cookie
}

/// Synchronizer-token CSRF validation backed by the server-side session.
#[derive(Clone, Copy, Debug, Default)]
pub struct Csrf;

impl Middleware for Csrf {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let Some(session) = request.extensions().get::<Session>().cloned() else {
                return Response::text("Session middleware is required")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            };
            let token = session
                .get("_csrf")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| {
                    let token = random_token(32);
                    session.put("_csrf", token.clone());
                    token
                });
            if !is_safe_method(request.method()) {
                let supplied = request
                    .headers()
                    .get("x-csrf-token")
                    .and_then(|value| value.to_str().ok());
                if !supplied.is_some_and(|supplied| constant_time_eq(supplied, &token)) {
                    return Response::text("CSRF token mismatch")
                        .with_status(StatusCode::FORBIDDEN);
                }
            }
            let mut response = next.run(request).await;
            if let Ok(value) = HeaderValue::from_str(&token) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static("x-csrf-token"), value);
            }
            response
        })
    }
}

fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// Configurable CSP, HSTS, and browser hardening headers.
#[derive(Clone, Debug)]
pub struct SecurityPolicy {
    pub content_security_policy: String,
    pub hsts: Option<Duration>,
    pub hsts_include_subdomains: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            content_security_policy:
                "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; object-src 'none'"
                    .to_owned(),
            hsts: Some(Duration::from_hours(8760)),
            hsts_include_subdomains: true,
        }
    }
}

impl Middleware for SecurityPolicy {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let policy = self.clone();
        Box::pin(async move {
            let mut response = next.run(request).await;
            insert_default(response.headers_mut(), "x-content-type-options", "nosniff");
            insert_default(response.headers_mut(), "x-frame-options", "DENY");
            insert_default(
                response.headers_mut(),
                "referrer-policy",
                "strict-origin-when-cross-origin",
            );
            insert_default(
                response.headers_mut(),
                "permissions-policy",
                "camera=(), microphone=(), geolocation=()",
            );
            insert_default(
                response.headers_mut(),
                "content-security-policy",
                &policy.content_security_policy,
            );
            if let Some(max_age) = policy.hsts {
                let mut value = format!("max-age={}", max_age.as_secs());
                if policy.hsts_include_subdomains {
                    value.push_str("; includeSubDomains");
                }
                insert_default(response.headers_mut(), "strict-transport-security", &value);
            }
            response
        })
    }
}

fn insert_default(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if !headers.contains_key(name)
        && let Ok(value) = HeaderValue::from_str(value)
    {
        headers.insert(HeaderName::from_static(name), value);
    }
}

/// Emits query-free structured access logs and never records header values.
#[derive(Clone, Copy, Debug, Default)]
pub struct AccessLog;

impl Middleware for AccessLog {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        let request_id = request
            .extensions()
            .get::<RequestIdValue>()
            .map_or_else(|| "missing".to_owned(), |value| value.0.clone());
        let client_ip = request.extensions().get::<ClientIp>().map(|value| value.0);
        Box::pin(async move {
            let started = Instant::now();
            let response = next.run(request).await;
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            tracing::info!(
                http.method = %method,
                http.path = %path,
                http.status = response.status().as_u16(),
                duration_ms,
                request_id,
                client_ip = ?client_ip,
                "request completed"
            );
            response
        })
    }
}

/// Return a diagnostic header map with credential-bearing values replaced.
#[must_use]
pub fn redact_headers(headers: &HeaderMap) -> HashMap<String, String> {
    const SENSITIVE: &[&str] = &[
        "authorization",
        "cookie",
        "set-cookie",
        "proxy-authorization",
        "x-api-key",
        "x-csrf-token",
    ];
    headers
        .iter()
        .map(|(name, value)| {
            let value = if SENSITIVE.contains(&name.as_str()) {
                "[REDACTED]".to_owned()
            } else {
                value.to_str().unwrap_or("[BINARY]").to_owned()
            };
            (name.as_str().to_owned(), value)
        })
        .collect()
}

fn random_token(bytes: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let random: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    let mut token = String::with_capacity(bytes * 2);
    for byte in random {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::{IntoResponse, Uri};
    use phoenix_routing::{Router, Routes};

    fn request(method: Method, path: &str) -> Request {
        Request::new(method, path.parse::<Uri>().unwrap())
    }

    async fn handle(router: &Router, request: Request) -> Response {
        router.handle(request).await
    }

    #[tokio::test]
    async fn session_cookie_and_csrf_round_trip() {
        let store = SessionStore::memory(Duration::from_hours(1));
        let router = Routes::new()
            .get("/", |request: Request| async move {
                request
                    .extensions()
                    .get::<Session>()
                    .unwrap()
                    .put("user", "Ada");
                "ok"
            })
            .post("/", |_request: Request| async { "changed" })
            .with_middleware(SessionMiddleware::new(store, SessionConfig::default()))
            .with_middleware(Csrf)
            .build()
            .unwrap();

        let response = handle(&router, request(Method::GET, "/")).await;
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        let csrf = response
            .headers()
            .get("x-csrf-token")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        let mut accepted = request(Method::POST, "/");
        accepted
            .headers_mut()
            .insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());
        accepted
            .headers_mut()
            .insert("x-csrf-token", HeaderValue::from_str(&csrf).unwrap());
        assert_eq!(handle(&router, accepted).await.status(), StatusCode::OK);

        let mut rejected = request(Method::POST, "/");
        rejected
            .headers_mut()
            .insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());
        assert_eq!(
            handle(&router, rejected).await.status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn trusts_forwarding_chain_only_from_configured_peer() {
        let proxy: IpAddr = "10.0.0.2".parse().unwrap();
        let trusted_hops = [
            proxy,
            "10.0.0.7".parse().unwrap(),
            "10.0.0.8".parse().unwrap(),
        ];
        let handler = |request: Request| async move {
            request
                .extensions()
                .get::<ClientIp>()
                .map_or_else(|| "missing".to_owned(), |client| client.0.to_string())
                .into_response()
        };
        let router = Routes::new()
            .get("/", handler)
            .with_middleware(TrustedProxies::new(trusted_hops))
            .build()
            .unwrap();

        let mut trusted = request(Method::GET, "/");
        trusted.extensions_mut().insert(SocketAddr::new(proxy, 443));
        trusted.headers_mut().insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.7, 10.0.0.2"),
        );
        assert_eq!(handle(&router, trusted).await.body(), "198.51.100.7");

        let mut untrusted = request(Method::GET, "/");
        untrusted
            .extensions_mut()
            .insert("203.0.113.9:1234".parse::<SocketAddr>().unwrap());
        untrusted
            .headers_mut()
            .insert("x-forwarded-for", HeaderValue::from_static("198.51.100.7"));
        assert_eq!(handle(&router, untrusted).await.body(), "203.0.113.9");

        let mut all_trusted = request(Method::GET, "/");
        all_trusted
            .extensions_mut()
            .insert(SocketAddr::new(proxy, 443));
        all_trusted.headers_mut().insert(
            "x-forwarded-for",
            HeaderValue::from_static("10.0.0.7, 10.0.0.8"),
        );
        assert_eq!(handle(&router, all_trusted).await.body(), "10.0.0.7");
    }

    #[tokio::test]
    async fn host_cors_rate_limit_and_security_headers_fail_closed() {
        let mut cors = CorsConfig::default();
        cors.allowed_origins
            .insert("https://app.invalid".to_owned());
        cors.allowed_methods.insert(Method::PUT);
        cors.allowed_headers.insert(header::AUTHORIZATION);
        let router = Routes::new()
            .get("/", |_request: Request| async { "ok" })
            .with_middleware(HostAllowlist::new(["app.invalid"]))
            .with_middleware(Cors::new(cors))
            .with_middleware(RateLimit::new(RateLimitConfig {
                requests: 1,
                window: Duration::from_mins(1),
            }))
            .with_middleware(SecurityPolicy::default())
            .build()
            .unwrap();

        let mut first = request(Method::GET, "/");
        first
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("app.invalid:443"));
        first
            .extensions_mut()
            .insert(ClientIp("127.0.0.1".parse().unwrap()));
        let response = handle(&router, first).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("content-security-policy"));
        assert!(response.headers().contains_key("strict-transport-security"));

        let mut limited = request(Method::GET, "/");
        limited
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("app.invalid"));
        limited
            .extensions_mut()
            .insert(ClientIp("127.0.0.1".parse().unwrap()));
        assert_eq!(
            handle(&router, limited).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );

        let mut bad_host = request(Method::GET, "/");
        bad_host
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("evil.invalid"));
        assert_eq!(
            handle(&router, bad_host).await.status(),
            StatusCode::BAD_REQUEST
        );

        let mut preflight = request(Method::OPTIONS, "/");
        preflight
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("app.invalid"));
        preflight.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.invalid"),
        );
        preflight.headers_mut().insert(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("PUT"),
        );
        preflight.headers_mut().insert(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            HeaderValue::from_static("authorization"),
        );
        let response = handle(&router, preflight).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://app.invalid"))
        );

        let mut disallowed_method = request(Method::DELETE, "/");
        disallowed_method
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("app.invalid"));
        disallowed_method.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.invalid"),
        );
        assert_eq!(
            handle(&router, disallowed_method).await.status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn generates_unique_request_ids_visible_to_handlers() {
        let router = Routes::new()
            .get("/", |request: Request| async move {
                request
                    .extensions()
                    .get::<RequestIdValue>()
                    .unwrap()
                    .0
                    .clone()
            })
            .with_middleware(RequestId)
            .build()
            .unwrap();
        let first = handle(&router, request(Method::GET, "/")).await;
        let second = handle(&router, request(Method::GET, "/")).await;
        assert_eq!(
            first.body().as_ref(),
            first.headers()["x-request-id"].as_bytes()
        );
        assert_ne!(first.body(), second.body());
    }

    #[test]
    fn redacts_credentials_but_keeps_safe_diagnostics() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        headers.insert(header::USER_AGENT, HeaderValue::from_static("test-client"));
        let redacted = redact_headers(&headers);
        assert_eq!(redacted["authorization"], "[REDACTED]");
        assert_eq!(redacted["user-agent"], "test-client");

        let store = SessionStore::memory(Duration::from_hours(1));
        let (session, _) = store.open(None);
        session.put("user", "Ada");
        let original_id = session.id();
        session.regenerate();
        assert_ne!(session.id(), original_id);
        assert_eq!(session.get("user"), Some(Value::String("Ada".to_owned())));
        let regenerated_id = session.id();
        session.invalidate();
        assert_ne!(session.id(), regenerated_id);
        assert_eq!(session.get("user"), None);
        let config = SessionConfig {
            secure: false,
            same_site: SameSite::None,
            ..SessionConfig::default()
        };
        assert!(session_cookie(&session, &config).contains("; Secure"));
    }
}
