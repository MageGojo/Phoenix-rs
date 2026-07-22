use std::{convert::Infallible, future::Future, net::SocketAddr, sync::Arc};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request as HyperRequest, Response as HyperResponse, body::Incoming, service::service_fn,
};
use hyper_util::rt::TokioIo;
use phoenix_http::{Request, Response};
use phoenix_routing::{RouteBuildError, Router, Routes};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{oneshot, watch},
    task::{JoinHandle, JoinSet},
};

const DEFAULT_MAX_BODY_SIZE: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct Application {
    router: Router,
    max_body_size: usize,
}

impl Application {
    /// Build an application from its route declarations.
    ///
    /// # Errors
    ///
    /// Returns a route build error when route patterns or names are invalid.
    pub fn new(routes: Routes) -> Result<Self, RouteBuildError> {
        Ok(Self {
            router: routes.build()?,
            max_body_size: DEFAULT_MAX_BODY_SIZE,
        })
    }

    #[must_use]
    pub fn max_body_size(mut self, bytes: usize) -> Self {
        self.max_body_size = bytes;
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

        while connections.join_next().await.is_some() {}
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
    let service = service_fn(move |request| {
        let application = Arc::clone(&application);
        async move { handle_hyper_request(application, request).await }
    });
    let io = TokioIo::new(stream);
    let builder = hyper::server::conn::http1::Builder::new();
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
) -> Result<HyperResponse<Full<Bytes>>, Infallible> {
    let (parts, body) = request.into_parts();
    let body = Limited::new(body, application.max_body_size)
        .collect()
        .await;

    let response = match body {
        Ok(body) => {
            let request =
                Request::from_parts(parts.method, parts.uri, parts.headers, body.to_bytes());
            application.handle(request).await
        }
        Err(_) => Response::new(http::StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"),
    };

    Ok(into_hyper_response(response))
}

fn into_hyper_response(response: Response) -> HyperResponse<Full<Bytes>> {
    let (status, headers, body) = response.into_parts();
    let mut response = HyperResponse::new(Full::new(body));
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
