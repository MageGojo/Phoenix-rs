use std::{
    convert::Infallible, future::Future, io::Cursor, net::SocketAddr, path::Path, sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
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
use phoenix_http::{
    ConnectionInfo, ConnectionUpgrade, Middleware, Request, RequestBodyError, RequestBodyMode,
    RequestBodyStream, Response, ResponseBody, ResponseBodyError, ResponseCancellationToken,
    StateMiddleware, TransportScheme,
};
use phoenix_metrics::Metrics;
use phoenix_routing::{MultiRouterError, RouteBuildError, RouteGroup, Router, RouterMount, Routes};
use rustls::ServerConfig;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::TlsAcceptor;

const DEFAULT_MAX_BODY_SIZE: usize = 2 * 1024 * 1024;
const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_REJECTED_BODY_DRAIN_SIZE: usize = 64 * 1024;
const MAX_REJECTED_BODY_DRAIN_SIZE: usize = 8 * 1024 * 1024;
const ALPN_HTTP_2: &[u8] = b"h2";
const ALPN_HTTP_1_1: &[u8] = b"http/1.1";

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

/// Rustls server configuration with Phoenix ALPN and handshake policy.
#[derive(Clone)]
pub struct TlsConfig {
    server_config: Arc<ServerConfig>,
    handshake_timeout: Duration,
}

impl TlsConfig {
    /// Load a certificate chain and private key from PEM bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable PEM, missing material, or an invalid key/certificate pair.
    pub fn from_pem(
        certificate_pem: &[u8],
        private_key_pem: &[u8],
    ) -> Result<Self, TlsConfigError> {
        let certificates = rustls_pemfile::certs(&mut Cursor::new(certificate_pem))
            .collect::<Result<Vec<_>, _>>()?;
        if certificates.is_empty() {
            return Err(TlsConfigError::MissingCertificate);
        }
        let private_key = rustls_pemfile::private_key(&mut Cursor::new(private_key_pem))?
            .ok_or(TlsConfigError::MissingPrivateKey)?;
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)?;
        Ok(Self::from_server_config(server_config))
    }

    /// Load PEM material from local files during application startup.
    ///
    /// # Errors
    ///
    /// Returns an error when either file cannot be read or parsed.
    pub fn from_files(
        certificate_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> Result<Self, TlsConfigError> {
        let certificate_pem = std::fs::read(certificate_path)?;
        let private_key_pem = std::fs::read(private_key_path)?;
        Self::from_pem(&certificate_pem, &private_key_pem)
    }

    /// Wrap an advanced rustls configuration. Phoenix supplies HTTP ALPN defaults when absent.
    #[must_use]
    pub fn from_server_config(mut server_config: ServerConfig) -> Self {
        if server_config.alpn_protocols.is_empty() {
            server_config.alpn_protocols = vec![ALPN_HTTP_2.to_vec(), ALPN_HTTP_1_1.to_vec()];
        }
        Self {
            server_config: Arc::new(server_config),
            handshake_timeout: DEFAULT_TLS_HANDSHAKE_TIMEOUT,
        }
    }

    /// Set a hard deadline for completing the TLS handshake.
    ///
    /// # Errors
    ///
    /// Returns an error when the timeout is zero.
    pub fn handshake_timeout(mut self, timeout: Duration) -> Result<Self, TlsConfigError> {
        if timeout.is_zero() {
            return Err(TlsConfigError::InvalidHandshakeTimeout);
        }
        self.handshake_timeout = timeout;
        Ok(self)
    }

    #[must_use]
    pub fn alpn_protocols(&self) -> &[Vec<u8>] {
        &self.server_config.alpn_protocols
    }

    fn for_http_protocol(&self, protocol: HttpProtocol) -> Self {
        let mut server_config = (*self.server_config).clone();
        server_config.alpn_protocols = match protocol {
            HttpProtocol::Auto => vec![ALPN_HTTP_2.to_vec(), ALPN_HTTP_1_1.to_vec()],
            HttpProtocol::Http1Only => vec![ALPN_HTTP_1_1.to_vec()],
            HttpProtocol::Http2Only => vec![ALPN_HTTP_2.to_vec()],
        };
        Self {
            server_config: Arc::new(server_config),
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TlsConfig")
            .field("alpn_protocols", &self.server_config.alpn_protocols)
            .field("handshake_timeout", &self.handshake_timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum TlsConfigError {
    #[error("TLS PEM I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS certificate PEM does not contain a certificate")]
    MissingCertificate,
    #[error("TLS private-key PEM does not contain a supported private key")]
    MissingPrivateKey,
    #[error("TLS certificate or private key is invalid: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("TLS handshake timeout must be greater than zero")]
    InvalidHandshakeTimeout,
}

#[derive(Clone)]
pub struct Application {
    router: Router,
    max_body_size: usize,
    header_read_timeout: Duration,
    body_read_timeout: Duration,
    graceful_shutdown_timeout: Duration,
    http_protocol: HttpProtocol,
    metrics: Option<Metrics>,
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
            metrics: None,
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

    /// Attach the process metrics registry used for connection and TLS telemetry.
    #[must_use]
    pub fn metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    #[must_use]
    pub fn router(&self) -> &Router {
        &self.router
    }

    pub async fn handle(&self, request: Request) -> Response {
        self.router.handle(request).await
    }

    fn request_body_mode(&self, request: &Request) -> RequestBodyMode {
        self.router.request_body_mode(request)
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
        self.bind_with_transport(address, ServerTransport::Plain)
            .await
    }

    /// Bind an HTTPS listener using rustls and ALPN.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the address cannot be resolved or bound.
    pub async fn bind_tls<A>(self, address: A, tls: TlsConfig) -> Result<Server, ServerError>
    where
        A: ToSocketAddrs,
    {
        let tls = tls.for_http_protocol(self.http_protocol);
        self.bind_with_transport(address, ServerTransport::Tls(tls))
            .await
    }

    async fn bind_with_transport<A>(
        self,
        address: A,
        transport: ServerTransport,
    ) -> Result<Server, ServerError>
    where
        A: ToSocketAddrs,
    {
        let listener = TcpListener::bind(address).await?;
        let local_addr = listener.local_addr()?;
        Ok(Server {
            application: Arc::new(self),
            listener,
            local_addr,
            transport,
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
        Ok(spawn_server(server))
    }

    /// Bind and run an HTTPS application in a Tokio task.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener cannot be created.
    pub async fn spawn_tls<A>(self, address: A, tls: TlsConfig) -> Result<ServerHandle, ServerError>
    where
        A: ToSocketAddrs,
    {
        let server = self.bind_tls(address, tls).await?;
        Ok(spawn_server(server))
    }
}

fn spawn_server(server: Server) -> ServerHandle {
    let local_addr = server.local_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        server
            .run_with_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });
    ServerHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        task,
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
    transport: ServerTransport,
}

#[derive(Clone)]
enum ServerTransport {
    Plain,
    Tls(TlsConfig),
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
        let response_cancellation = ResponseCancellationToken::new();
        let mut connections = JoinSet::new();
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                () = &mut shutdown => {
                    response_cancellation.cancel();
                    let _ = connection_shutdown_tx.send(true);
                    break;
                }
                completed = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = completed {
                        tracing::warn!(error = %error, "HTTP connection task failed");
                    }
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted?;
                    let application = Arc::clone(&self.application);
                    let connection_shutdown = connection_shutdown_tx.subscribe();
                    let response_cancellation = response_cancellation.clone();
                    let transport = self.transport.clone();
                    connections.spawn(async move {
                        serve_transport(
                            stream,
                            application,
                            connection_shutdown,
                            response_cancellation,
                            transport,
                        ).await;
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

async fn serve_transport(
    stream: TcpStream,
    application: Arc<Application>,
    shutdown: watch::Receiver<bool>,
    response_cancellation: ResponseCancellationToken,
    transport: ServerTransport,
) {
    let _connection_guard = application.metrics.as_ref().map(Metrics::connection_opened);
    let peer_addr = stream.peer_addr().ok();
    match transport {
        ServerTransport::Plain => {
            let connection_info = ConnectionInfo::new(peer_addr, TransportScheme::Http, None);
            serve_connection(
                stream,
                application,
                shutdown,
                response_cancellation,
                connection_info,
            )
            .await;
        }
        ServerTransport::Tls(config) => {
            let acceptor = TlsAcceptor::from(Arc::clone(&config.server_config));
            match tokio::time::timeout(config.handshake_timeout, acceptor.accept(stream)).await {
                Ok(Ok(tls_stream)) => {
                    if let Some(metrics) = &application.metrics {
                        metrics.record_tls_handshake(true);
                    }
                    let alpn = tls_stream
                        .get_ref()
                        .1
                        .alpn_protocol()
                        .map(|protocol| String::from_utf8_lossy(protocol).into_owned());
                    let connection_info =
                        ConnectionInfo::new(peer_addr, TransportScheme::Https, alpn);
                    serve_connection(
                        tls_stream,
                        application,
                        shutdown,
                        response_cancellation,
                        connection_info,
                    )
                    .await;
                }
                Ok(Err(error)) => {
                    if let Some(metrics) = &application.metrics {
                        metrics.record_tls_handshake(false);
                    }
                    tracing::warn!(peer = ?peer_addr, error = %error, "TLS handshake failed");
                }
                Err(_) => {
                    if let Some(metrics) = &application.metrics {
                        metrics.record_tls_handshake(false);
                    }
                    tracing::warn!(peer = ?peer_addr, "TLS handshake timed out");
                }
            }
        }
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

async fn serve_connection<I>(
    stream: I,
    application: Arc<Application>,
    mut shutdown: watch::Receiver<bool>,
    response_cancellation: ResponseCancellationToken,
    connection_info: ConnectionInfo,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let header_read_timeout = application.header_read_timeout;
    let http_protocol = application.http_protocol;
    let service = service_fn(move |request| {
        let application = Arc::clone(&application);
        let connection_info = connection_info.clone();
        let response_cancellation = response_cancellation.clone();
        async move {
            handle_hyper_request(application, request, connection_info, response_cancellation).await
        }
    });
    let io = TokioIo::new(stream);

    // `auto::Builder::serve_connection_with_upgrades` ignores http1_only/http2_only, so
    // protocol-restricted modes use dedicated builders. Auto still upgrades HTTP/1.
    match http_protocol {
        HttpProtocol::Http1Only => {
            let mut http1 = hyper::server::conn::http1::Builder::new();
            http1.timer(TokioTimer::new());
            http1.header_read_timeout(header_read_timeout);
            let connection = http1.serve_connection(io, service).with_upgrades();
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
        HttpProtocol::Http2Only => {
            let mut builder = auto::Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout);
            let builder = builder.http2_only();
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
        HttpProtocol::Auto => {
            let mut builder = auto::Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout);
            let connection = builder.serve_connection_with_upgrades(io, service);
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
    }
}

async fn handle_hyper_request(
    application: Arc<Application>,
    request: HyperRequest<Incoming>,
    connection_info: ConnectionInfo,
    response_cancellation: ResponseCancellationToken,
) -> Result<HyperResponse<UnsyncBoxBody<Bytes, ResponseBodyError>>, Infallible> {
    let (parts, body) = request.into_parts();
    let declared_body_too_large =
        declared_content_length_exceeds(&parts.headers, application.max_body_size);
    let mut request = Request::from_server_parts(
        parts.method,
        parts.uri,
        parts.version,
        parts.headers,
        parts.extensions,
        Bytes::new(),
    );
    if let Some(on_upgrade) = request
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>()
    {
        request
            .extensions_mut()
            .insert(ConnectionUpgrade::new(on_upgrade));
    }
    if let Some(peer_addr) = connection_info.peer_addr() {
        request.extensions_mut().insert(peer_addr);
    }
    request.extensions_mut().insert(connection_info);

    let response = if declared_body_too_large {
        drain_rejected_body(
            body,
            application.max_body_size,
            application.body_read_timeout,
        );
        Response::text("Payload Too Large").with_status(http::StatusCode::PAYLOAD_TOO_LARGE)
    } else {
        match application.request_body_mode(&request) {
            RequestBodyMode::Streaming => {
                request.set_streaming_body(incoming_body_stream(
                    body,
                    application.max_body_size,
                    application.body_read_timeout,
                ));
                application.handle(request).await
            }
            RequestBodyMode::Buffered => {
                let body = tokio::time::timeout(
                    application.body_read_timeout,
                    Limited::new(body, application.max_body_size).collect(),
                )
                .await;
                match body {
                    Ok(Ok(body)) => {
                        request.set_buffered_body(body.to_bytes());
                        application.handle(request).await
                    }
                    Ok(Err(error)) if error.is::<LengthLimitError>() => {
                        Response::text("Payload Too Large")
                            .with_status(http::StatusCode::PAYLOAD_TOO_LARGE)
                    }
                    Ok(Err(_)) => {
                        Response::text("Bad Request").with_status(http::StatusCode::BAD_REQUEST)
                    }
                    Err(_) => Response::text("Request Timeout")
                        .with_status(http::StatusCode::REQUEST_TIMEOUT),
                }
            }
        }
    };

    Ok(into_hyper_response(response, response_cancellation))
}

fn drain_rejected_body(body: Incoming, max_body_size: usize, body_read_timeout: Duration) {
    let drain_limit = max_body_size
        .saturating_mul(2)
        .clamp(MIN_REJECTED_BODY_DRAIN_SIZE, MAX_REJECTED_BODY_DRAIN_SIZE);
    drop(tokio::spawn(async move {
        let _ = tokio::time::timeout(body_read_timeout, Limited::new(body, drain_limit).collect())
            .await;
    }));
}

fn declared_content_length_exceeds(headers: &http::HeaderMap, max_body_size: usize) -> bool {
    let max_body_size = u64::try_from(max_body_size).unwrap_or(u64::MAX);
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|content_length| content_length > max_body_size)
}

fn incoming_body_stream(
    body: Incoming,
    max_body_size: usize,
    body_read_timeout: Duration,
) -> RequestBodyStream {
    let body = Limited::new(body, max_body_size).into_data_stream();
    let deadline = tokio::time::Instant::now() + body_read_timeout;
    RequestBodyStream::new(stream::unfold(
        Some((body, deadline)),
        move |state| async move {
            let (mut body, deadline) = state?;
            match tokio::time::timeout_at(deadline, body.next()).await {
                Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some((body, deadline)))),
                Ok(Some(Err(error))) if error.is::<LengthLimitError>() => {
                    Some((Err(RequestBodyError::TooLarge), None))
                }
                Ok(Some(Err(_))) => Some((Err(RequestBodyError::Transport), None)),
                Ok(None) => None,
                Err(_) => Some((Err(RequestBodyError::Timeout(body_read_timeout)), None)),
            }
        },
    ))
}

fn into_hyper_response(
    response: Response,
    response_cancellation: ResponseCancellationToken,
) -> HyperResponse<UnsyncBoxBody<Bytes, ResponseBodyError>> {
    let (status, headers, body) = response.into_network_parts(response_cancellation);
    let body = match body {
        ResponseBody::Buffered(bytes) => Full::new(bytes)
            .map_err(|error: Infallible| match error {})
            .boxed_unsync(),
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

/// Build a multi-application server from concise module declarations.
///
/// Each entry expands to [`ApplicationModule`] builder calls and the macro
/// returns `Result<Application, MultiApplicationError>`.
///
/// ```
/// use phoenix_runtime::applications;
/// use phoenix_routing::Routes;
///
/// let application = applications! {
///     website => Routes::new().get("/", |_request: phoenix_http::Request| async { "site" }), [root];
///     admin => Routes::new().get("/", |_request: phoenix_http::Request| async { "admin" }), [];
/// };
/// assert!(application.is_ok());
/// ```
#[macro_export]
macro_rules! applications {
    (
        $(
            $name:ident => $routes:expr, [$($options:tt)*];
        )+
    ) => {{
        let builder = $crate::Application::multi();
        $(
            let module = $crate::ApplicationModule::new(stringify!($name), $routes);
            let module = $crate::__phoenix_application_module!(module; $($options)*);
            let builder = builder.mount(module);
        )+
        builder.build()
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __phoenix_application_module {
    ($module:expr;) => { $module };
    ($module:expr; root $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.root(); $($($rest)*)?)
    };
    ($module:expr; prefix = $prefix:expr $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.prefix($prefix); $($($rest)*)?)
    };
    ($module:expr; host = $host:expr $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.host($host); $($($rest)*)?)
    };
    ($module:expr; name_prefix = $prefix:expr $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.name_prefix($prefix); $($($rest)*)?)
    };
    ($module:expr; middleware = $middleware:expr $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.middleware($middleware); $($($rest)*)?)
    };
    ($module:expr; state = $state:expr $(, $($rest:tt)*)?) => {
        $crate::__phoenix_application_module!($module.state($state); $($($rest)*)?)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt as _, StreamExt, stream};
    use http_body_util::{Empty, StreamBody, combinators::UnsyncBoxBody};
    use hyper::client::conn::http2;
    use phoenix_http::{
        CloseCode, CloseFrame, IntoResponse, KeepAlive, Method, RequestBodyError,
        RequestBodyStream, Sse, SseEvent, State, WebSocketUpgrade, streaming, typed,
    };
    use phoenix_routing::ApplicationContext;
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
    use std::{
        future::{Ready, ready},
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;
    use tokio_rustls::TlsConnector;
    use tokio_tungstenite::{
        client_async,
        tungstenite::{self, client::IntoClientRequest, protocol::Message as TungsteniteMessage},
    };

    fn streaming_handler(_request: Request) -> Ready<Response> {
        ready(Response::stream(stream::iter([
            Bytes::from_static(b"first-"),
            Bytes::from_static(b"second"),
        ])))
    }

    async fn read_http1_until(client: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
        let mut response = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buffer))
                .await
                .expect("HTTP/1 response timed out")
                .expect("HTTP/1 response read failed");
            if read == 0 {
                break;
            }
            response.extend_from_slice(&buffer[..read]);
            if response
                .windows(needle.len())
                .any(|window| window == needle)
            {
                break;
            }
        }
        response
    }

    async fn read_http1_to_end(client: &mut TcpStream) -> Vec<u8> {
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
            .await
            .expect("HTTP/1 response timed out")
            .expect("HTTP/1 response read failed");
        response
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
    async fn streaming_request_yields_a_chunk_before_upload_completion() {
        let handler = streaming(typed(|mut body: RequestBodyStream| async move {
            match body.next_chunk().await {
                Some(Ok(chunk)) => Ok(chunk),
                Some(Err(error)) => Err(error),
                None => Ok(Bytes::new()),
            }
        }));
        let server = Application::new(Routes::new().post("/upload", handler))
            .unwrap()
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n",
            )
            .await
            .unwrap();

        let response = read_http1_until(&mut client, b"first").await;
        assert!(response.starts_with(b"HTTP/1.1 200 OK"));
        assert!(response.windows(5).any(|window| window == b"first"));

        drop(client);
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn declared_streaming_body_limit_rejects_before_handler_side_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = Arc::clone(&calls);
        let handler = streaming(move |_request: Request| {
            let handler_calls = Arc::clone(&handler_calls);
            async move {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                "unexpected"
            }
        });
        let server = Application::new(Routes::new().post("/upload", handler))
            .unwrap()
            .max_body_size(4)
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde",
            )
            .await
            .unwrap();

        let response = read_http1_to_end(&mut client).await;
        assert!(response.starts_with(b"HTTP/1.1 413 Payload Too Large"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn chunked_streaming_body_limit_maps_to_payload_too_large() {
        let handler = streaming(typed(|body: RequestBodyStream| async move {
            body.into_bytes().await.map(|_| "accepted")
        }));
        let server = Application::new(Routes::new().post("/upload", handler))
            .unwrap()
            .max_body_size(4)
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n",
            )
            .await
            .unwrap();

        let response = read_http1_to_end(&mut client).await;
        assert!(response.starts_with(b"HTTP/1.1 413 Payload Too Large"));

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn streaming_body_uses_an_absolute_read_deadline() {
        let handler = streaming(typed(|mut body: RequestBodyStream| async move {
            match body.next_chunk().await {
                Some(Err(error)) => Err(error),
                Some(Ok(_)) | None => Ok("unexpected"),
            }
        }));
        let server = Application::new(Routes::new().post("/upload", handler))
            .unwrap()
            .body_read_timeout(Duration::from_millis(50))
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let response = read_http1_to_end(&mut client).await;
        assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout"));

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn streaming_handler_observes_client_disconnect() {
        let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
        let handler = streaming(typed(move |mut body: RequestBodyStream| {
            let observed_tx = observed_tx.clone();
            async move {
                let disconnected = matches!(
                    body.next_chunk().await,
                    Some(Err(RequestBodyError::Transport))
                );
                let _ = observed_tx.send(disconnected);
                "finished"
            }
        }));
        let server = Application::new(Routes::new().post("/upload", handler))
            .unwrap()
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
        drop(client);

        let disconnected = tokio::time::timeout(Duration::from_secs(2), observed_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(disconnected);

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dropping_stream_before_eof_does_not_corrupt_http1_pipeline() {
        let upload = streaming(typed(|mut body: RequestBodyStream| async move {
            let _ = body.next_chunk().await;
            "upload-finished"
        }));
        let routes = Routes::new()
            .post("/upload", upload)
            .get("/health", |_request: Request| async { "healthy" });
        let server = Application::new(routes)
            .unwrap()
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n0\r\n\r\nGET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let response = read_http1_to_end(&mut client).await;
        let response = String::from_utf8(response).unwrap();
        assert_eq!(response.matches("HTTP/1.1 200 OK").count(), 2);
        assert!(response.contains("upload-finished"));
        assert!(response.contains("healthy"));

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn http2_keeps_concurrent_streams_healthy_during_streaming_upload() {
        let (started_tx, started_rx) = oneshot::channel();
        let started_tx = Arc::new(Mutex::new(Some(started_tx)));
        let release = Arc::new(Notify::new());
        let handler_started = Arc::clone(&started_tx);
        let handler_release = Arc::clone(&release);
        let upload = streaming(typed(move |mut body: RequestBodyStream| {
            let handler_started = Arc::clone(&handler_started);
            let handler_release = Arc::clone(&handler_release);
            async move {
                let _ = body.next_chunk().await;
                if let Some(started_tx) = handler_started.lock().unwrap().take() {
                    let _ = started_tx.send(());
                }
                handler_release.notified().await;
                "upload-finished"
            }
        }));
        let routes =
            Routes::new()
                .post("/upload", upload)
                .get("/health", |request: Request| async move {
                    assert_eq!(request.version(), http::Version::HTTP_2);
                    "healthy"
                });
        let server = Application::new(routes)
            .unwrap()
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let stream = TcpStream::connect(server.local_addr()).await.unwrap();
        let (mut sender, connection) = http2::handshake::<_, _, UnsyncBoxBody<Bytes, Infallible>>(
            TokioExecutor::new(),
            TokioIo::new(stream),
        )
        .await
        .unwrap();
        let connection_task = tokio::spawn(connection);
        let upload_body = StreamBody::new(
            stream::iter([Ok::<_, Infallible>(Frame::data(Bytes::from_static(
                b"first",
            )))])
            .chain(stream::pending()),
        )
        .boxed_unsync();
        let upload_request = HyperRequest::builder()
            .method(http::Method::POST)
            .uri("http://localhost/upload")
            .body(upload_body)
            .unwrap();
        let upload_response = sender.send_request(upload_request);
        tokio::pin!(upload_response);

        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        let health_request = HyperRequest::builder()
            .uri("http://localhost/health")
            .body(Empty::new().boxed_unsync())
            .unwrap();
        let health_response = sender.send_request(health_request).await.unwrap();
        assert_eq!(
            health_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
            "healthy"
        );

        release.notify_waiters();
        let upload_response = upload_response.await.unwrap();
        assert_eq!(
            upload_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
            "upload-finished"
        );

        drop(sender);
        server.shutdown().await.unwrap();
        connection_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn graceful_shutdown_aborts_a_stalled_streaming_upload_at_deadline() {
        let (started_tx, started_rx) = oneshot::channel();
        let started_tx = Arc::new(Mutex::new(Some(started_tx)));
        let handler_started = Arc::clone(&started_tx);
        let upload = streaming(typed(move |mut body: RequestBodyStream| {
            let handler_started = Arc::clone(&handler_started);
            async move {
                if let Some(started_tx) = handler_started.lock().unwrap().take() {
                    let _ = started_tx.send(());
                }
                let _ = body.next_chunk().await;
                "finished"
            }
        }));
        let server = Application::new(Routes::new().post("/upload", upload))
            .unwrap()
            .body_read_timeout(Duration::from_secs(30))
            .graceful_shutdown_timeout(Duration::from_millis(50))
            .spawn("127.0.0.1:0")
            .await
            .unwrap();
        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();

        tokio::time::timeout(Duration::from_millis(500), server.shutdown())
            .await
            .expect("server shutdown exceeded its hard deadline")
            .unwrap();
        drop(client);
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

    #[tokio::test]
    async fn tls_listener_negotiates_http2_with_alpn_and_exposes_secure_connection_info() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let tls = TlsConfig::from_pem(
            cert.pem().as_bytes(),
            signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
        assert_eq!(
            tls.alpn_protocols(),
            &[ALPN_HTTP_2.to_vec(), ALPN_HTTP_1_1.to_vec()]
        );
        let metrics = Metrics::new();
        let server =
            Application::new(Routes::new().get("/secure", |request: Request| async move {
                let info = request.extensions().get::<ConnectionInfo>().unwrap();
                format!(
                    "{}:{}",
                    info.scheme().as_str(),
                    info.alpn_protocol().unwrap_or("missing")
                )
            }))
            .unwrap()
            .metrics(metrics.clone())
            .spawn_tls("127.0.0.1:0", tls)
            .await
            .unwrap();

        let mut roots = RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        let mut client_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client_config.alpn_protocols = vec![ALPN_HTTP_2.to_vec()];
        let connector = TlsConnector::from(Arc::new(client_config));
        let stream = TcpStream::connect(server.local_addr()).await.unwrap();
        let tls_stream = connector
            .connect(ServerName::try_from("localhost").unwrap(), stream)
            .await
            .unwrap();
        assert_eq!(tls_stream.get_ref().1.alpn_protocol(), Some(ALPN_HTTP_2));

        let (mut sender, connection) =
            http2::handshake::<_, _, Empty<Bytes>>(TokioExecutor::new(), TokioIo::new(tls_stream))
                .await
                .unwrap();
        let connection_task = tokio::spawn(connection);
        let request = HyperRequest::builder()
            .uri("https://localhost/secure")
            .body(Empty::new())
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.version(), http::Version::HTTP_2);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "https:h2"
        );

        drop(sender);
        server.shutdown().await.unwrap();
        connection_task.await.unwrap().unwrap();
        let exported = metrics.render();
        assert!(exported.contains("phoenix_connections_total 1"));
        assert!(exported.contains("phoenix_connections_active 0"));
        assert!(exported.contains("outcome=\"success\"} 1"));
    }

    #[test]
    fn tls_configuration_rejects_missing_material_and_zero_deadlines() {
        assert!(matches!(
            TlsConfig::from_pem(b"", b""),
            Err(TlsConfigError::MissingCertificate)
        ));
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let tls = TlsConfig::from_pem(
            cert.pem().as_bytes(),
            signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            tls.handshake_timeout(Duration::ZERO),
            Err(TlsConfigError::InvalidHandshakeTimeout)
        ));
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

    #[tokio::test]
    async fn sse_tcp_delivers_events_keepalives_and_unblocks_shutdown_on_disconnect() {
        let finished = Arc::new(Notify::new());
        let hang_finished = Arc::clone(&finished);
        let routes = Routes::new()
            .get("/events", |_request: Request| async {
                Sse::from_events(stream::iter([SseEvent::new().data("hello-sse")]))
            })
            .get("/keepalive", |_request: Request| async {
                Sse::from_events(stream::pending()).keep_alive(
                    KeepAlive::new(Duration::from_millis(20))
                        .unwrap()
                        .comment("tick")
                        .unwrap(),
                )
            })
            .get("/hang", move |_request: Request| {
                let finished = Arc::clone(&hang_finished);
                async move {
                    let mut response =
                        Sse::from_events(stream::pending::<SseEvent>()).into_response();
                    response.on_body_finish(move |_| {
                        finished.notify_waiters();
                    });
                    response
                }
            });

        let server = Application::new(routes)
            .unwrap()
            .graceful_shutdown_timeout(Duration::from_secs(2))
            .spawn("127.0.0.1:0")
            .await
            .unwrap();

        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let body = read_http1_to_end(&mut client).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("HTTP/1.1 200 OK"));
        assert!(body.contains("text/event-stream"));
        assert!(body.contains("data: hello-sse"));

        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(b"GET /keepalive HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let keepalive = read_http1_until(&mut client, b": tick").await;
        assert!(keepalive.windows(6).any(|window| window == b": tick"));
        drop(client);

        let mut client = TcpStream::connect(server.local_addr()).await.unwrap();
        client
            .write_all(b"GET /hang HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let _ = read_http1_until(&mut client, b"text/event-stream").await;
        drop(client);
        tokio::time::timeout(Duration::from_secs(2), finished.notified())
            .await
            .expect("SSE stream should finish after client disconnect");

        tokio::time::timeout(Duration::from_secs(2), server.shutdown())
            .await
            .expect("graceful shutdown should not block on finished SSE streams")
            .unwrap();
    }

    fn websocket_test_routes() -> Routes {
        Routes::new()
            .get(
                "/echo",
                typed(|ws: WebSocketUpgrade| async move {
                    ws.any_origin().on_upgrade(|mut socket| async move {
                        while let Some(message) = socket.recv().await {
                            let Ok(message) = message else { break };
                            if message.is_close() {
                                let _ = socket
                                    .close(Some(CloseFrame {
                                        code: CloseCode::NORMAL,
                                        reason: String::new(),
                                    }))
                                    .await;
                                break;
                            }
                            if socket.send(message).await.is_err() {
                                break;
                            }
                        }
                    })
                }),
            )
            .get(
                "/origin",
                typed(|ws: WebSocketUpgrade| async move {
                    ws.allowed_origin("https://app.example")
                        .on_upgrade(|_socket| async {})
                }),
            )
            .get(
                "/small",
                typed(|ws: WebSocketUpgrade| async move {
                    ws.any_origin()
                        .max_message_size(8)
                        .unwrap()
                        .max_frame_size(8)
                        .unwrap()
                        .on_upgrade(|mut socket| async move {
                            while let Some(message) = socket.recv().await {
                                match message {
                                    Ok(message) if !message.is_close() => {}
                                    _ => break,
                                }
                            }
                        })
                }),
            )
    }

    async fn spawn_websocket_server() -> ServerHandle {
        Application::new(websocket_test_routes())
            .unwrap()
            .http_protocol(HttpProtocol::Http1Only)
            .spawn("127.0.0.1:0")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn websocket_tcp_echo_and_graceful_close() {
        let server = spawn_websocket_server().await;
        let addr = server.local_addr();

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut request = format!("ws://{addr}/echo").into_client_request().unwrap();
        request.headers_mut().insert(
            http::header::ORIGIN,
            http::HeaderValue::from_static("http://localhost"),
        );
        let (mut socket, response) = client_async(request, stream).await.unwrap();
        assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
        socket
            .send(TungsteniteMessage::Text("ping-ws".into()))
            .await
            .unwrap();
        let echoed = socket.next().await.unwrap().unwrap();
        assert_eq!(echoed.into_text().unwrap(), "ping-ws");
        socket
            .close(Some(tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "".into(),
            }))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), server.shutdown())
            .await
            .expect("server shutdown after websocket echo")
            .unwrap();
    }

    #[tokio::test]
    async fn websocket_tcp_rejects_disallowed_origin() {
        let server = spawn_websocket_server().await;
        let addr = server.local_addr();

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut request = format!("ws://{addr}/origin").into_client_request().unwrap();
        request.headers_mut().insert(
            http::header::ORIGIN,
            http::HeaderValue::from_static("https://evil.example"),
        );
        let error = client_async(request, stream).await.unwrap_err();
        let tungstenite::Error::Http(response) = error else {
            panic!("expected HTTP rejection, got {error:?}");
        };
        assert_eq!(response.status(), http::StatusCode::FORBIDDEN);

        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_tcp_closes_on_oversized_message() {
        let server = spawn_websocket_server().await;
        let addr = server.local_addr();

        let stream = TcpStream::connect(addr).await.unwrap();
        let request = format!("ws://{addr}/small").into_client_request().unwrap();
        let (mut socket, _) = client_async(request, stream).await.unwrap();
        socket
            .send(TungsteniteMessage::Text("x".repeat(64).into()))
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match socket.next().await {
                    Some(Ok(TungsteniteMessage::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "oversized message should close the socket");

        server.shutdown().await.unwrap();
    }
}
