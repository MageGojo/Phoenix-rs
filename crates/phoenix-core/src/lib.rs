use std::{convert::Infallible, future::Future, net::SocketAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{
    BodyExt, Full, LengthLimitError, Limited, StreamBody, combinators::UnsyncBoxBody,
};
use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::{Frame, Incoming},
    service::service_fn,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto,
};
use phoenix_http::{Middleware, Request, Response, ResponseBody, StateMiddleware};
use phoenix_routing::{MultiRouterError, RouteBuildError, RouteGroup, Router, RouterMount, Routes};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{oneshot, watch},
    task::{JoinHandle, JoinSet},
};

const DEFAULT_MAX_BODY_SIZE: usize = 2 * 1024 * 1024;
const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP protocol versions accepted by the built-in TCP server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HttpProtocol {
    /// Detect HTTP/1.1 or HTTP/2 from the connection preface.
    #[default]
    Auto,
    /// Accept only HTTP/1.1 connections.
    Http1Only,
    /// Accept only HTTP/2 connections.
    Http2Only,
}

#[derive(Clone)]
pub struct Application {
    router: Router,
    max_body_size: usize,
    header_read_timeout: Duration,
    body_read_timeout: Duration,
    graceful_shutdown_timeout: Duration,
    http_protocol: HttpProtocol,
}

impl Application {
    /// Build an application from its route declarations.
    ///
    /// # Errors
    ///
    /// Returns a route build error when route patterns or names are invalid.
    pub fn new(routes: Routes) -> Result<Self, RouteBuildError> {
        Ok(Self::from_router(routes.build()?))
    }

    fn from_router(router: Router) -> Self {
        Self {
            router,
            max_body_size: DEFAULT_MAX_BODY_SIZE,
            header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
            body_read_timeout: DEFAULT_BODY_READ_TIMEOUT,
            graceful_shutdown_timeout: DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
            http_protocol: HttpProtocol::Auto,
        }
    }

    /// Start building an application composed of independently scoped modules.
    #[must_use]
    pub fn multi() -> MultiApplicationBuilder {
        MultiApplicationBuilder::new()
    }

    #[must_use]
    pub fn max_body_size(mut self, bytes: usize) -> Self {
        self.max_body_size = bytes;
        self
    }

    #[must_use]
    pub fn header_read_timeout(mut self, timeout: Duration) -> Self {
        self.header_read_timeout = timeout;
        self
    }

    #[must_use]
    pub fn body_read_timeout(mut self, timeout: Duration) -> Self {
        self.body_read_timeout = timeout;
        self
    }

    #[must_use]
    pub fn graceful_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.graceful_shutdown_timeout = timeout;
        self
    }

    /// Select which HTTP protocol versions the built-in TCP server accepts.
    #[must_use]
    pub const fn http_protocol(mut self, protocol: HttpProtocol) -> Self {
        self.http_protocol = protocol;
        self
    }

    #[must_use]
    pub fn router(&self) -> &Router {
        &self.router
    }

    pub async fn handle(&self, request: Request) -> Response {
        self.router.handle(request).await
    }

    /// Bind the application to a TCP address without accepting connections yet.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the address cannot be resolved or bound.
    pub async fn bind<A>(self, address: A) -> Result<Server, ServerError>
    where
        A: ToSocketAddrs,
    {
        let listener = TcpListener::bind(address).await?;
        let local_addr = listener.local_addr()?;
        Ok(Server {
            application: Arc::new(self),
            listener,
            local_addr,
        })
    }

    /// Bind and run the application in a Tokio task.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener cannot be created.
    pub async fn spawn<A>(self, address: A) -> Result<ServerHandle, ServerError>
    where
        A: ToSocketAddrs,
    {
        let server = self.bind(address).await?;
        let local_addr = server.local_addr();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .run_with_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });

        Ok(ServerHandle {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            task,
        })
    }
}

/// One website/API/admin module mounted into the same Phoenix process.
pub struct ApplicationModule {
    name: String,
    path_prefix: String,
    name_prefix: String,
    host: Option<String>,
    routes: Routes,
}

impl ApplicationModule {
    /// Create a module mounted at `/{name}` with route names prefixed by `{name}.`.
    #[must_use]
    pub fn new(name: impl Into<String>, routes: Routes) -> Self {
        let name = name.into();
        Self {
            path_prefix: format!("/{name}"),
            name_prefix: format!("{name}."),
            name,
            host: None,
            routes,
        }
    }

    /// Mount this module at `/`.
    #[must_use]
    pub fn root(mut self) -> Self {
        "/".clone_into(&mut self.path_prefix);
        self
    }

    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = prefix.into();
        self
    }

    /// Restrict this module to a Host authority. A host without a port matches any port.
    #[must_use]
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Override the automatic `{application}.` named-route prefix.
    #[must_use]
    pub fn name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.name_prefix = prefix.into();
        self
    }

    /// Apply middleware only to routes in this application module.
    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        self.routes = self.routes.with_middleware(middleware);
        self
    }

    /// Insert cloneable, strongly typed state only for this application module.
    #[must_use]
    pub fn state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.middleware(StateMiddleware::new(state))
    }
}

/// Builds a single server from multiple isolated application modules.
#[derive(Default)]
pub struct MultiApplicationBuilder {
    modules: Vec<ApplicationModule>,
}

impl MultiApplicationBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn mount(mut self, module: ApplicationModule) -> Self {
        self.modules.push(module);
        self
    }

    /// Compile all application routers and the Host/path dispatcher.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid routes, application selectors, or global route names.
    pub fn build(self) -> Result<Application, MultiApplicationError> {
        let mounts = self
            .modules
            .into_iter()
            .map(|module| {
                let scoped_routes = module.routes.scoped(
                    RouteGroup::new()
                        .prefix(&module.path_prefix)
                        .name(&module.name_prefix),
                );
                let compiled_router = scoped_routes.build()?;
                RouterMount::new(
                    module.name,
                    module.path_prefix,
                    module.host,
                    compiled_router,
                )
                .map_err(MultiApplicationError::Router)
            })
            .collect::<Result<Vec<_>, MultiApplicationError>>()?;
        Ok(Application::from_router(Router::multi(mounts)?))
    }
}

#[derive(Debug, Error)]
pub enum MultiApplicationError {
    #[error(transparent)]
    Route(#[from] RouteBuildError),
    #[error(transparent)]
    Router(#[from] MultiRouterError),
}

pub struct Server {
    application: Arc<Application>,
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl Server {
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept connections until the task is cancelled.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when accepting a connection fails.
    pub async fn run(self) -> Result<(), ServerError> {
        self.run_with_shutdown(std::future::pending::<()>()).await
    }

    /// Accept connections until `shutdown` resolves, then gracefully close them.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when accepting a connection fails.
    pub async fn run_with_shutdown<F>(self, shutdown: F) -> Result<(), ServerError>
    where
        F: Future<Output = ()> + Send,
    {
        let (connection_shutdown_tx, _) = watch::channel(false);
        let mut connections = JoinSet::new();
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                () = &mut shutdown => {
                    let _ = connection_shutdown_tx.send(true);
                    break;
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted?;
                    let application = Arc::clone(&self.application);
                    let connection_shutdown = connection_shutdown_tx.subscribe();
                    connections.spawn(async move {
                        serve_connection(stream, application, connection_shutdown).await;
                    });
                }
            }
        }

        let graceful_shutdown_timeout = self.application.graceful_shutdown_timeout;
        let drained = tokio::time::timeout(graceful_shutdown_timeout, async {
            while connections.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }
        Ok(())
    }
}

pub struct ServerHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl ServerHandle {
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signal the spawned server to stop and wait for its task to finish.
    ///
    /// # Errors
    ///
    /// Returns an error when the server task fails or encounters an I/O error.
    pub async fn shutdown(mut self) -> Result<(), ServerError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.task.await??;
        Ok(())
    }
}

async fn serve_connection(
    stream: TcpStream,
    application: Arc<Application>,
    mut shutdown: watch::Receiver<bool>,
) {
    let peer_addr = stream.peer_addr().ok();
    let header_read_timeout = application.header_read_timeout;
    let http_protocol = application.http_protocol;
    let service = service_fn(move |request| {
        let application = Arc::clone(&application);
        async move { handle_hyper_request(application, request, peer_addr).await }
    });
    let io = TokioIo::new(stream);
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    let builder = match http_protocol {
        HttpProtocol::Auto => builder,
        HttpProtocol::Http1Only => builder.http1_only(),
        HttpProtocol::Http2Only => builder.http2_only(),
    };
    let connection = builder.serve_connection(io, service);
    tokio::pin!(connection);

    tokio::select! {
        _ = &mut connection => {}
        changed = shutdown.changed() => {
            if changed.is_ok() && *shutdown.borrow() {
                connection.as_mut().graceful_shutdown();
                let _ = connection.await;
            }
        }
    }
}

async fn handle_hyper_request(
    application: Arc<Application>,
    request: HyperRequest<Incoming>,
    peer_addr: Option<SocketAddr>,
) -> Result<HyperResponse<UnsyncBoxBody<Bytes, Infallible>>, Infallible> {
    let (parts, body) = request.into_parts();
    let body = tokio::time::timeout(
        application.body_read_timeout,
        Limited::new(body, application.max_body_size).collect(),
    )
    .await;

    let response = match body {
        Ok(Ok(body)) => {
            let mut request =
                Request::from_parts(parts.method, parts.uri, parts.headers, body.to_bytes());
            if let Some(peer_addr) = peer_addr {
                request.extensions_mut().insert(peer_addr);
            }
            application.handle(request).await
        }
        Ok(Err(error)) if error.is::<LengthLimitError>() => {
            Response::text("Payload Too Large").with_status(http::StatusCode::PAYLOAD_TOO_LARGE)
        }
        Ok(Err(_)) => Response::text("Bad Request").with_status(http::StatusCode::BAD_REQUEST),
        Err(_) => Response::text("Request Timeout").with_status(http::StatusCode::REQUEST_TIMEOUT),
    };

    Ok(into_hyper_response(response))
}

fn into_hyper_response(response: Response) -> HyperResponse<UnsyncBoxBody<Bytes, Infallible>> {
    let (status, headers, body) = response.into_parts();
    let body = match body {
        ResponseBody::Buffered(bytes) => Full::new(bytes).boxed_unsync(),
        ResponseBody::Stream(stream) => StreamBody::new(stream.map_ok(Frame::data)).boxed_unsync(),
    };
    let mut response = HyperResponse::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("server task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use http_body_util::Empty;
    use hyper::client::conn::http2;
    use phoenix_http::{Method, State, typed};
    use phoenix_routing::ApplicationContext;
    use std::future::{Ready, ready};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn streaming_handler(_request: Request) -> Ready<Response> {
        ready(Response::stream(stream::iter([
            Bytes::from_static(b"first-"),
            Bytes::from_static(b"second"),
        ])))
    }

    #[tokio::test]
    async fn hyper_forwards_response_chunks_without_content_length() {
        let server = Application::new(Routes::new().get("/stream", streaming_handler))
            .unwrap()
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("transfer-encoding: chunked"));
        assert!(!response.contains("content-length:"));
        assert!(response.contains("first-"));
        assert!(response.contains("second"));

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn auto_protocol_serves_http2_over_cleartext() {
        let server =
            Application::new(Routes::new().get("/version", |_request: Request| async { "h2" }))
                .unwrap()
                .spawn("127.0.0.1:0")
                .await
                .unwrap();
        let stream = TcpStream::connect(server.local_addr()).await.unwrap();
        let (mut sender, connection) =
            http2::handshake::<_, _, Empty<Bytes>>(TokioExecutor::new(), TokioIo::new(stream))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let request = HyperRequest::builder()
            .uri("http://localhost/version")
            .body(Empty::new())
            .unwrap();

        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.version(), http::Version::HTTP_2);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "h2"
        );

        drop(sender);
        server.shutdown().await.unwrap();
        connection_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn protocol_can_be_restricted_to_http1() {
        let server = Application::new(Routes::new().get("/", |_request: Request| async { "ok" }))
            .unwrap()
            .http_protocol(HttpProtocol::Http1Only)
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let stream = TcpStream::connect(server.local_addr()).await.unwrap();
        let (mut sender, connection) =
            http2::handshake::<_, _, Empty<Bytes>>(TokioExecutor::new(), TokioIo::new(stream))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let request = HyperRequest::builder()
            .uri("http://localhost/")
            .body(Empty::new())
            .unwrap();
        assert!(sender.send_request(request).await.is_err());
        connection_task.await.unwrap().unwrap_err();
        server.shutdown().await.unwrap();
    }

    #[derive(Clone)]
    struct ModuleLabel(&'static str);

    fn module_routes(route_name: &'static str) -> Routes {
        Routes::new()
            .get(
                "/",
                typed(
                    |State(state): State<ModuleLabel>,
                     State(context): State<ApplicationContext>| async move {
                        format!("{}:{}:{}", context.name(), context.path_prefix(), state.0)
                    },
                ),
            )
            .name(route_name)
    }

    #[tokio::test]
    async fn multi_application_mounts_root_path_and_host_modules_with_isolated_state() {
        let application = Application::multi()
            .mount(
                ApplicationModule::new("site", module_routes("home"))
                    .root()
                    .state(ModuleLabel("public")),
            )
            .mount(
                ApplicationModule::new("admin", module_routes("dashboard"))
                    .state(ModuleLabel("staff")),
            )
            .mount(
                ApplicationModule::new("partner", module_routes("home"))
                    .root()
                    .host("partner.test")
                    .state(ModuleLabel("partner")),
            )
            .build()
            .unwrap();

        let site = application
            .handle(Request::new(Method::GET, "/".parse().unwrap()))
            .await;
        assert_eq!(site.body(), "site:/:public");

        let admin = application
            .handle(Request::new(Method::GET, "/admin".parse().unwrap()))
            .await;
        assert_eq!(admin.body(), "admin:/admin:staff");

        let mut partner_request = Request::new(Method::GET, "/".parse().unwrap());
        partner_request.headers_mut().insert(
            http::header::HOST,
            http::HeaderValue::from_static("partner.test"),
        );
        let partner = application.handle(partner_request).await;
        assert_eq!(partner.body(), "partner:/:partner");

        assert_eq!(application.router().url("site.home", &[]).unwrap(), "/");
        assert_eq!(
            application.router().url("admin.dashboard", &[]).unwrap(),
            "/admin"
        );
        assert_eq!(application.router().url("partner.home", &[]).unwrap(), "/");
    }
}
