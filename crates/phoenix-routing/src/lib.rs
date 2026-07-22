use std::{collections::HashMap, fmt, panic::AssertUnwindSafe, sync::Arc};

use bytes::Bytes;
use futures_util::FutureExt;
use http::{HeaderValue, Method, StatusCode};
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

impl fmt::Debug for Router {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Router")
            .field("named_routes", &self.named_routes)
            .finish_non_exhaustive()
    }
}

impl Router {
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
pub enum UrlGenerationError {
    #[error("unknown named route `{0}`")]
    UnknownRoute(String),
    #[error("named route `{route}` requires parameter `{parameter}`")]
    MissingParameter { route: String, parameter: String },
}
