use std::{
    collections::{HashMap, HashSet},
    fmt,
    panic::AssertUnwindSafe,
    sync::Arc,
};

use bytes::Bytes;
use futures_util::FutureExt;
pub use http::Method;
use http::{HeaderValue, StatusCode, header, uri::Authority};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use phoenix_http::{
    Handler, IntoResponse, Middleware, Request, Response, RouteManifest, apply_middleware,
};
use thiserror::Error;

const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

type MatchedRoute = (usize, Vec<(String, String)>);

#[derive(Clone, Copy, Debug)]
struct InvalidPathParameter;

struct RouteDefinition {
    method: Method,
    path: String,
    name: Option<String>,
    handler: Arc<dyn Handler>,
    middleware: Vec<Arc<dyn Middleware>>,
}

#[derive(Default)]
pub struct Routes {
    definitions: Vec<RouteDefinition>,
    global_middleware: Vec<Arc<dyn Middleware>>,
    error: Option<RouteBuildError>,
}

impl Routes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn get<H>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.route(Method::GET, path, handler)
    }

    #[must_use]
    pub fn post<H>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.route(Method::POST, path, handler)
    }

    #[must_use]
    pub fn put<H>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.route(Method::PUT, path, handler)
    }

    #[must_use]
    pub fn patch<H>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.route(Method::PATCH, path, handler)
    }

    #[must_use]
    pub fn delete<H>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.route(Method::DELETE, path, handler)
    }

    #[must_use]
    pub fn route<H>(mut self, method: Method, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler,
    {
        self.definitions.push(RouteDefinition {
            method,
            path: normalize_path(&path.into()),
            name: None,
            handler: Arc::new(handler),
            middleware: Vec::new(),
        });
        self
    }

    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        if let Some(route) = self.definitions.last_mut() {
            route.name = Some(name.into());
        } else {
            self.error = Some(RouteBuildError::NoRouteToConfigure("name"));
        }
        self
    }

    /// Bind browser action input and output contracts to the most recently
    /// declared route. The Rust types are consumed by Phoenix's build tooling;
    /// request extraction and response serialization remain compiler checked
    /// by the registered handler.
    #[must_use]
    pub fn action<Input, Output>(mut self) -> Self {
        if self.definitions.last().is_none() {
            self.error = Some(RouteBuildError::NoRouteToConfigure("action"));
        }
        self
    }

    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        if let Some(route) = self.definitions.last_mut() {
            route.middleware.push(Arc::new(middleware));
        } else {
            self.error = Some(RouteBuildError::NoRouteToConfigure("middleware"));
        }
        self
    }

    #[must_use]
    pub fn with_middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        self.global_middleware.push(Arc::new(middleware));
        self
    }

    /// Merge declarations from another route file while preserving their
    /// registration order. Global middleware from `other` remains scoped to
    /// the imported declarations.
    #[must_use]
    pub fn merge(mut self, mut other: Self) -> Self {
        if self.error.is_none() {
            self.error = other.error.take();
        }
        let imported_globals = other.global_middleware;
        for mut definition in other.definitions {
            let mut middleware = imported_globals.clone();
            middleware.append(&mut definition.middleware);
            definition.middleware = middleware;
            self.definitions.push(definition);
        }
        self
    }

    #[must_use]
    pub fn group<F>(mut self, group: RouteGroup, configure: F) -> Self
    where
        F: FnOnce(Routes) -> Routes,
    {
        let RouteGroup {
            prefix,
            name_prefix,
            middleware: group_middleware,
        } = group;
        let child = configure(Self::new());
        if self.error.is_none() {
            self.error = child.error;
        }

        for mut route in child.definitions {
            route.path = join_paths(&prefix, &route.path);
            route.name = route.name.map(|name| format!("{name_prefix}{name}"));

            let mut middleware = group_middleware.clone();
            middleware.extend(child.global_middleware.iter().cloned());
            middleware.extend(route.middleware);
            route.middleware = middleware;
            self.definitions.push(route);
        }
        self
    }

    /// Apply a path/name/middleware scope to an existing route collection.
    #[must_use]
    pub fn scoped(self, group: RouteGroup) -> Self {
        Self::new().group(group, |_| self)
    }

    /// Compile route definitions into an immutable request router.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid patterns, duplicate names, or configuration
    /// calls that did not target a registered route.
    pub fn build(self) -> Result<Router, RouteBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }

        let mut method_routers: HashMap<Method, matchit::Router<usize>> = HashMap::new();
        let mut named_routes = HashMap::new();
        let mut compiled_routes = Vec::with_capacity(self.definitions.len());

        for definition in self.definitions {
            let index = compiled_routes.len();
            let method = definition.method.clone();
            let path = definition.path.clone();

            method_routers
                .entry(method.clone())
                .or_default()
                .insert(path.clone(), index)
                .map_err(|error| RouteBuildError::InvalidPattern {
                    method: method.clone(),
                    path: path.clone(),
                    reason: error.to_string(),
                })?;

            if let Some(name) = &definition.name
                && named_routes.insert(name.clone(), path.clone()).is_some()
            {
                return Err(RouteBuildError::DuplicateName(name.clone()));
            }

            compiled_routes.push(CompiledRoute {
                name: definition.name,
                handler: apply_middleware(definition.handler, &definition.middleware),
            });
        }

        let inner = Arc::new(RouterInner {
            method_routers,
            routes: compiled_routes,
        });
        let dispatch: Arc<dyn Handler> = Arc::new(DispatchHandler {
            inner: Arc::clone(&inner),
        });
        let dispatch: Arc<dyn Handler> = Arc::new(PanicBoundary { next: dispatch });
        let handler = apply_middleware(dispatch, &self.global_middleware);

        Ok(Router {
            handler: Arc::new(PanicBoundary { next: handler }),
            named_routes: Arc::new(named_routes),
        })
    }
}

#[derive(Clone, Default)]
pub struct RouteGroup {
    prefix: String,
    name_prefix: String,
    middleware: Vec<Arc<dyn Middleware>>,
}

impl RouteGroup {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = normalize_prefix(&prefix.into());
        self
    }

    #[must_use]
    pub fn name(mut self, prefix: impl Into<String>) -> Self {
        self.name_prefix = prefix.into();
        self
    }

    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        self.middleware.push(Arc::new(middleware));
        self
    }
}

#[derive(Clone)]
pub struct Router {
    handler: Arc<dyn Handler>,
    named_routes: Arc<HashMap<String, String>>,
}

/// Metadata for the application module selected for the current request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationContext {
    name: Arc<str>,
    path_prefix: Arc<str>,
    host: Option<Arc<str>>,
}

impl ApplicationContext {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn path_prefix(&self) -> &str {
        &self.path_prefix
    }

    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }
}

/// One independently compiled router mounted into a multi-application router.
pub struct RouterMount {
    context: ApplicationContext,
    match_prefix: String,
    authority: Option<Authority>,
    router: Router,
}

impl RouterMount {
    /// Construct and validate a router mount.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid application names or Host authorities.
    pub fn new(
        name: impl Into<String>,
        path_prefix: impl Into<String>,
        host: Option<impl Into<String>>,
        router: Router,
    ) -> Result<Self, MultiRouterError> {
        let name = name.into();
        if !valid_application_name(&name) {
            return Err(MultiRouterError::InvalidApplicationName(name));
        }
        let match_prefix = normalize_prefix(&path_prefix.into());
        let display_prefix = if match_prefix.is_empty() {
            "/".to_owned()
        } else {
            match_prefix.clone()
        };
        let (authority, display_host) = match host.map(Into::into) {
            Some(host) => {
                let normalized = host.trim().to_ascii_lowercase();
                let authority = normalized
                    .parse::<Authority>()
                    .map_err(|_| MultiRouterError::InvalidHost(host))?;
                (Some(authority), Some(Arc::from(normalized)))
            }
            None => (None, None),
        };
        Ok(Self {
            context: ApplicationContext {
                name: Arc::from(name),
                path_prefix: Arc::from(display_prefix),
                host: display_host,
            },
            match_prefix,
            authority,
            router,
        })
    }
}

impl fmt::Debug for Router {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Router")
            .field("named_routes", &self.named_routes)
            .finish_non_exhaustive()
    }
}

impl Router {
    /// Compose independently built routers using Host and path-prefix selection.
    ///
    /// Host-bound mounts win over hostless fallbacks, explicit ports win over
    /// host-only bindings, and longer path prefixes win over shorter prefixes.
    ///
    /// # Errors
    ///
    /// Returns an error for empty sets, duplicate application names/selectors,
    /// or duplicate global route names.
    pub fn multi(mounts: Vec<RouterMount>) -> Result<Self, MultiRouterError> {
        if mounts.is_empty() {
            return Err(MultiRouterError::Empty);
        }
        let mut application_names = HashSet::new();
        let mut selectors = HashSet::new();
        let mut named_routes = HashMap::new();
        for mount in &mounts {
            if !application_names.insert(mount.context.name.to_string()) {
                return Err(MultiRouterError::DuplicateApplication(
                    mount.context.name.to_string(),
                ));
            }
            let selector = (
                mount.authority.as_ref().map(ToString::to_string),
                mount.match_prefix.clone(),
            );
            if !selectors.insert(selector.clone()) {
                return Err(MultiRouterError::DuplicateSelector {
                    host: selector.0,
                    path_prefix: display_prefix(&selector.1),
                });
            }
            for (name, path) in mount.router.named_routes.iter() {
                if named_routes.insert(name.clone(), path.clone()).is_some() {
                    return Err(MultiRouterError::DuplicateRouteName(name.clone()));
                }
            }
        }
        let dispatch: Arc<dyn Handler> = Arc::new(MultiDispatch { mounts });
        Ok(Self {
            handler: Arc::new(PanicBoundary { next: dispatch }),
            named_routes: Arc::new(named_routes),
        })
    }

    pub async fn handle(&self, mut request: Request) -> Response {
        request
            .extensions_mut()
            .insert(RouteManifest::new(Arc::clone(&self.named_routes)));
        self.handler.call(request).await
    }

    /// Generate a URL from a Laravel-style named route.
    ///
    /// # Errors
    ///
    /// Returns an error when the route name is unknown or a required path
    /// parameter is missing.
    pub fn url(&self, name: &str, params: &[(&str, &str)]) -> Result<String, UrlGenerationError> {
        let pattern = self
            .named_routes
            .get(name)
            .ok_or_else(|| UrlGenerationError::UnknownRoute(name.to_owned()))?;
        let params: HashMap<&str, &str> = params.iter().copied().collect();
        let mut output = String::with_capacity(pattern.len());

        for segment in pattern.split('/') {
            if segment.is_empty() {
                continue;
            }
            output.push('/');
            if let Some(parameter) = segment
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
            {
                let value =
                    params
                        .get(parameter)
                        .ok_or_else(|| UrlGenerationError::MissingParameter {
                            route: name.to_owned(),
                            parameter: parameter.to_owned(),
                        })?;
                output.push_str(&utf8_percent_encode(value, PATH_SEGMENT_ENCODE_SET).to_string());
            } else {
                output.push_str(segment);
            }
        }

        if output.is_empty() {
            output.push('/');
        }
        Ok(output)
    }
}

struct MultiDispatch {
    mounts: Vec<RouterMount>,
}

impl Handler for MultiDispatch {
    fn call(&self, mut request: Request) -> phoenix_http::BoxFuture<Response> {
        let selected = select_mount(&self.mounts, &request)
            .map(|mount| (mount.context.clone(), Arc::clone(&mount.router.handler)));
        Box::pin(async move {
            let Some((context, handler)) = selected else {
                return (StatusCode::NOT_FOUND, "Application Not Found").into_response();
            };
            request.extensions_mut().insert(context);
            handler.call(request).await
        })
    }
}

fn select_mount<'a>(mounts: &'a [RouterMount], request: &Request) -> Option<&'a RouterMount> {
    let request_authority = request_authority(request);
    mounts
        .iter()
        .filter(|mount| {
            prefix_matches(&mount.match_prefix, request.uri().path())
                && mount.authority.as_ref().is_none_or(|configured| {
                    request_authority
                        .as_ref()
                        .is_some_and(|actual| authority_matches(configured, actual))
                })
        })
        .max_by_key(|mount| {
            (
                u8::from(mount.authority.is_some()),
                u8::from(
                    mount
                        .authority
                        .as_ref()
                        .is_some_and(|authority| authority.port_u16().is_some()),
                ),
                mount.match_prefix.len(),
            )
        })
}

fn request_authority(request: &Request) -> Option<Authority> {
    request.uri().authority().cloned().or_else(|| {
        request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.to_ascii_lowercase().parse().ok())
    })
}

fn authority_matches(configured: &Authority, actual: &Authority) -> bool {
    configured.host().eq_ignore_ascii_case(actual.host())
        && configured
            .port_u16()
            .is_none_or(|port| actual.port_u16() == Some(port))
}

fn prefix_matches(prefix: &str, path: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn valid_application_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn display_prefix(prefix: &str) -> String {
    if prefix.is_empty() {
        "/".to_owned()
    } else {
        prefix.to_owned()
    }
}

struct CompiledRoute {
    name: Option<String>,
    handler: Arc<dyn Handler>,
}

struct RouterInner {
    method_routers: HashMap<Method, matchit::Router<usize>>,
    routes: Vec<CompiledRoute>,
}

struct DispatchHandler {
    inner: Arc<RouterInner>,
}

struct PanicBoundary {
    next: Arc<dyn Handler>,
}

impl Handler for PanicBoundary {
    fn call(&self, request: Request) -> phoenix_http::BoxFuture<Response> {
        let next = Arc::clone(&self.next);
        Box::pin(async move {
            let result = AssertUnwindSafe(async move { next.call(request).await })
                .catch_unwind()
                .await;
            result.unwrap_or_else(|_| {
                Response::text("Internal Server Error")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR)
            })
        })
    }
}

impl Handler for DispatchHandler {
    fn call(&self, request: Request) -> phoenix_http::BoxFuture<Response> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move { inner.dispatch(request).await })
    }
}

impl RouterInner {
    async fn dispatch(&self, mut request: Request) -> Response {
        let is_head = request.method() == Method::HEAD;
        let lookup_method = if is_head && !self.has_match(&Method::HEAD, request.uri().path()) {
            &Method::GET
        } else {
            request.method()
        };

        match self.find_match(lookup_method, request.uri().path()) {
            Ok(Some((index, params))) => {
                let route = &self.routes[index];
                request.set_route(route.name.clone(), params);
                let response = route.handler.call(request).await;
                return if is_head {
                    let (status, headers, _) = response.into_parts();
                    let mut response = Response::new(status, Bytes::new());
                    *response.headers_mut() = headers;
                    response
                } else {
                    response
                };
            }
            Err(InvalidPathParameter) => {
                return (StatusCode::BAD_REQUEST, "Invalid path parameter encoding")
                    .into_response();
            }
            Ok(None) => {}
        }

        let allowed = self.allowed_methods(request.uri().path());
        if request.method() == Method::OPTIONS && !allowed.is_empty() {
            return response_with_allow(StatusCode::NO_CONTENT, &allowed);
        }
        if !allowed.is_empty() {
            return response_with_allow(StatusCode::METHOD_NOT_ALLOWED, &allowed);
        }

        (StatusCode::NOT_FOUND, "Not Found").into_response()
    }

    fn has_match(&self, method: &Method, path: &str) -> bool {
        self.method_routers
            .get(method)
            .is_some_and(|router| router.at(path).is_ok())
    }

    fn find_match(
        &self,
        method: &Method,
        path: &str,
    ) -> Result<Option<MatchedRoute>, InvalidPathParameter> {
        let Some(router) = self.method_routers.get(method) else {
            return Ok(None);
        };
        let Ok(matched) = router.at(path) else {
            return Ok(None);
        };
        let params = matched
            .params
            .iter()
            .map(|(key, value)| decode_path_parameter(value).map(|value| (key.to_owned(), value)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((*matched.value, params)))
    }

    fn allowed_methods(&self, path: &str) -> Vec<Method> {
        let mut methods: Vec<Method> = self
            .method_routers
            .iter()
            .filter(|(_, router)| router.at(path).is_ok())
            .map(|(method, _)| method.clone())
            .collect();
        if methods.contains(&Method::GET) && !methods.contains(&Method::HEAD) {
            methods.push(Method::HEAD);
        }
        methods.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        methods
    }
}

fn decode_path_parameter(value: &str) -> Result<String, InvalidPathParameter> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(encoded) = bytes.get(index + 1..index + 3) else {
                return Err(InvalidPathParameter);
            };
            if !encoded.iter().all(u8::is_ascii_hexdigit) {
                return Err(InvalidPathParameter);
            }
            index += 3;
        } else {
            index += 1;
        }
    }

    percent_decode_str(value)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
        .map_err(|_| InvalidPathParameter)
}

fn response_with_allow(status: StatusCode, methods: &[Method]) -> Response {
    let allow = methods
        .iter()
        .map(Method::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let mut response = Response::new(status, Bytes::new());
    if let Ok(value) = HeaderValue::from_str(&allow) {
        response.headers_mut().insert(http::header::ALLOW, value);
    }
    response
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/".to_owned()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    }
}

fn normalize_prefix(prefix: &str) -> String {
    let prefix = normalize_path(prefix);
    if prefix == "/" { String::new() } else { prefix }
}

fn join_paths(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        return normalize_path(path);
    }
    if path == "/" {
        return prefix.to_owned();
    }
    format!("{prefix}{}", normalize_path(path))
}

#[derive(Debug, Error)]
pub enum RouteBuildError {
    #[error("cannot configure route {0}: no route has been registered")]
    NoRouteToConfigure(&'static str),
    #[error("duplicate route name `{0}`")]
    DuplicateName(String),
    #[error("invalid route pattern for {method} {path}: {reason}")]
    InvalidPattern {
        method: Method,
        path: String,
        reason: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MultiRouterError {
    #[error("a multi-application router requires at least one application")]
    Empty,
    #[error("invalid application name `{0}`")]
    InvalidApplicationName(String),
    #[error("invalid application Host authority `{0}`")]
    InvalidHost(String),
    #[error("duplicate application name `{0}`")]
    DuplicateApplication(String),
    #[error("duplicate application selector for host {host:?} and path `{path_prefix}`")]
    DuplicateSelector {
        host: Option<String>,
        path_prefix: String,
    },
    #[error("duplicate route name `{0}` across applications")]
    DuplicateRouteName(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlGenerationError {
    #[error("unknown named route `{0}`")]
    UnknownRoute(String),
    #[error("named route `{route}` requires parameter `{parameter}`")]
    MissingParameter { route: String, parameter: String },
}

/// Declare a route collection without repeating the builder variable.
///
/// ```
/// use phoenix_routing::routes;
///
/// let routes = routes! {
///     GET "/" => |_request: phoenix_http::Request| async { "home" }, name = "home";
///     POST "/users" => |_request: phoenix_http::Request| async { "created" },
///         name = "users.store";
/// };
/// assert!(routes.build().is_ok());
/// ```
#[macro_export]
macro_rules! routes {
    (
        $(
            $method:ident $path:literal => $handler:expr
            $(, name = $name:literal)?
            $(, middleware = [$($middleware:expr),* $(,)?])?
            ;
        )*
    ) => {{
        let routes = $crate::Routes::new();
        $(
            let routes = $crate::__phoenix_route!(routes, $method, $path, $handler);
            $(let routes = routes.name($name);)?
            $($(let routes = routes.middleware($middleware);)*)?
        )*
        routes
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __phoenix_route {
    ($routes:expr, GET, $path:expr, $handler:expr) => {
        $routes.get($path, $handler)
    };
    ($routes:expr, POST, $path:expr, $handler:expr) => {
        $routes.post($path, $handler)
    };
    ($routes:expr, PUT, $path:expr, $handler:expr) => {
        $routes.put($path, $handler)
    };
    ($routes:expr, PATCH, $path:expr, $handler:expr) => {
        $routes.patch($path, $handler)
    };
    ($routes:expr, DELETE, $path:expr, $handler:expr) => {
        $routes.delete($path, $handler)
    };
    ($routes:expr, HEAD, $path:expr, $handler:expr) => {
        $routes.route($crate::Method::HEAD, $path, $handler)
    };
    ($routes:expr, OPTIONS, $path:expr, $handler:expr) => {
        $routes.route($crate::Method::OPTIONS, $path, $handler)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::SecurityHeaders;

    fn application_name(request: Request) -> phoenix_http::BoxFuture<Response> {
        Box::pin(async move {
            request
                .extensions()
                .get::<ApplicationContext>()
                .map_or_else(|| "missing".to_owned(), |context| context.name().to_owned())
                .into_response()
        })
    }

    #[tokio::test]
    async fn multi_router_selects_path_and_host_apps_and_generates_global_urls() {
        let site = Routes::new()
            .get("/", application_name)
            .name("home")
            .scoped(RouteGroup::new().name("site."))
            .build()
            .unwrap();
        let admin = Routes::new()
            .get("/", application_name)
            .name("dashboard")
            .scoped(RouteGroup::new().prefix("/admin").name("admin."))
            .build()
            .unwrap();
        let partner = Routes::new()
            .get("/", application_name)
            .name("home")
            .scoped(RouteGroup::new().name("partner."))
            .build()
            .unwrap();
        let router = Router::multi(vec![
            RouterMount::new("site", "/", None::<String>, site).unwrap(),
            RouterMount::new("admin", "/admin", None::<String>, admin).unwrap(),
            RouterMount::new("partner", "/", Some("partner.test"), partner).unwrap(),
        ])
        .unwrap();

        let site_response = router
            .handle(Request::new(Method::GET, "/".parse().unwrap()))
            .await;
        assert_eq!(site_response.body(), "site");

        let admin_response = router
            .handle(Request::new(Method::GET, "/admin".parse().unwrap()))
            .await;
        assert_eq!(admin_response.body(), "admin");

        let mut partner_request = Request::new(Method::GET, "/".parse().unwrap());
        partner_request
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("partner.test:8080"));
        let partner_response = router.handle(partner_request).await;
        assert_eq!(partner_response.body(), "partner");

        assert_eq!(router.url("site.home", &[]).unwrap(), "/");
        assert_eq!(router.url("admin.dashboard", &[]).unwrap(), "/admin");
        assert_eq!(router.url("partner.home", &[]).unwrap(), "/");
    }

    #[test]
    fn multi_router_rejects_ambiguous_apps() {
        let first = Routes::new().get("/", application_name).build().unwrap();
        let second = Routes::new().get("/", application_name).build().unwrap();
        let error = Router::multi(vec![
            RouterMount::new("first", "/same", None::<String>, first).unwrap(),
            RouterMount::new("second", "/same", None::<String>, second).unwrap(),
        ])
        .unwrap_err();
        assert_eq!(
            error,
            MultiRouterError::DuplicateSelector {
                host: None,
                path_prefix: "/same".to_owned(),
            }
        );
    }

    #[test]
    fn path_prefix_matching_stops_at_segment_boundaries() {
        assert!(prefix_matches("/admin", "/admin"));
        assert!(prefix_matches("/admin", "/admin/users"));
        assert!(!prefix_matches("/admin", "/administrator"));
    }

    #[tokio::test]
    async fn routes_macro_applies_names_methods_and_route_middleware() {
        let router = routes! {
            GET "/items" => |_request: Request| async { "items" }, name = "items.index";
            POST "/items" => |_request: Request| async { "created" },
                name = "items.store", middleware = [SecurityHeaders];
        }
        .build()
        .unwrap();

        let response = router
            .handle(Request::new(Method::POST, "/items".parse().unwrap()))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "created");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(router.url("items.index", &[]).unwrap(), "/items");
        assert_eq!(router.url("items.store", &[]).unwrap(), "/items");
    }
}
