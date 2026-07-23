//! Request test client and assertions for Phoenix applications.
//!
//! See `docs/TESTING_AND_STORAGE.md` and `docs/DX.md` §10.

#![forbid(unsafe_code)]

mod cookie;
mod request;
mod response;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use cookie::CookieJar;
use phoenix_runtime::{Application, ServerError, ServerHandle};
use phoenix_routing::{RouteBuildError, Routes};
use serde::Serialize;
use thiserror::Error;

pub use request::RequestBuilder;
pub use response::TestResponse;

/// Errors while starting or driving a [`TestApp`].
#[derive(Debug, Error)]
pub enum TestAppError {
    /// Route compilation failed before the server could bind.
    #[error(transparent)]
    Route(#[from] RouteBuildError),
    /// The ephemeral listener or server task failed.
    #[error(transparent)]
    Server(#[from] ServerError),
}

/// Values that can be turned into a runnable [`Application`] for tests.
pub trait IntoApplication {
    /// Build an [`Application`] ready to bind.
    ///
    /// # Errors
    ///
    /// Returns a route build error when declarations are invalid.
    fn into_application(self) -> Result<Application, RouteBuildError>;
}

impl IntoApplication for Application {
    fn into_application(self) -> Result<Application, RouteBuildError> {
        Ok(self)
    }
}

impl IntoApplication for Routes {
    fn into_application(self) -> Result<Application, RouteBuildError> {
        Application::new(self)
    }
}

impl IntoApplication for Result<Application, RouteBuildError> {
    fn into_application(self) -> Result<Application, RouteBuildError> {
        self
    }
}

/// In-process Phoenix application bound to an ephemeral local port.
pub struct TestApp {
    address: SocketAddr,
    server: Option<ServerHandle>,
    cookies: Arc<Mutex<CookieJar>>,
}

impl TestApp {
    /// Bind `target` on `127.0.0.1:0` and return a request client.
    ///
    /// Accepts [`Routes`], [`Application`], or `Result<Application, _>` so call
    /// sites can write either `TestApp::spawn(routes)` or
    /// `TestApp::spawn(Application::new(routes))`.
    ///
    /// # Panics
    ///
    /// Panics when the application cannot be built or the listener cannot bind.
    pub async fn spawn(target: impl IntoApplication) -> Self {
        Self::try_spawn(target)
            .await
            .expect("TestApp failed to start on an ephemeral port")
    }

    /// Fallible variant of [`Self::spawn`].
    ///
    /// # Errors
    ///
    /// Returns route build or server bind errors instead of panicking.
    pub async fn try_spawn(target: impl IntoApplication) -> Result<Self, TestAppError> {
        let application = target.into_application()?;
        let server = application.spawn("127.0.0.1:0").await?;
        Ok(Self {
            address: server.local_addr(),
            server: Some(server),
            cookies: Arc::new(Mutex::new(CookieJar::default())),
        })
    }

    /// Local socket address of the spawned server.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Base URL such as `http://127.0.0.1:54321`.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Start a GET request.
    #[must_use]
    pub fn get(&self, path: impl Into<String>) -> RequestBuilder {
        RequestBuilder::new(self, http::Method::GET, path.into())
    }

    /// Start a POST request without a body.
    #[must_use]
    pub fn post(&self, path: impl Into<String>) -> RequestBuilder {
        RequestBuilder::new(self, http::Method::POST, path.into())
    }

    /// Start a POST request with a JSON body.
    ///
    /// # Panics
    ///
    /// Panics when `body` cannot be serialized to JSON.
    #[must_use]
    pub fn post_json<T: Serialize>(&self, path: impl Into<String>, body: &T) -> RequestBuilder {
        let bytes = serde_json::to_vec(body).expect("test JSON body should serialize");
        RequestBuilder::new(self, http::Method::POST, path.into())
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(bytes)
    }

    /// Start a POST request with an `application/x-www-form-urlencoded` body.
    ///
    /// # Panics
    ///
    /// Panics when `body` cannot be serialized as a form.
    #[must_use]
    pub fn post_form<T: Serialize>(&self, path: impl Into<String>, body: &T) -> RequestBuilder {
        let encoded = serde_urlencoded::to_string(body).expect("test form body should serialize");
        RequestBuilder::new(self, http::Method::POST, path.into())
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(encoded.into_bytes())
    }

    /// Start a PUT request.
    #[must_use]
    pub fn put(&self, path: impl Into<String>) -> RequestBuilder {
        RequestBuilder::new(self, http::Method::PUT, path.into())
    }

    /// Start a PATCH request.
    #[must_use]
    pub fn patch(&self, path: impl Into<String>) -> RequestBuilder {
        RequestBuilder::new(self, http::Method::PATCH, path.into())
    }

    /// Start a DELETE request.
    #[must_use]
    pub fn delete(&self, path: impl Into<String>) -> RequestBuilder {
        RequestBuilder::new(self, http::Method::DELETE, path.into())
    }

    /// Signal shutdown and wait for the server task to finish.
    ///
    /// # Errors
    ///
    /// Returns a server error when the task fails during shutdown.
    pub async fn shutdown(mut self) -> Result<(), ServerError> {
        if let Some(server) = self.server.take() {
            server.shutdown().await?;
        }
        Ok(())
    }

    pub(crate) fn cookies(&self) -> Arc<Mutex<CookieJar>> {
        Arc::clone(&self.cookies)
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        // Dropping `ServerHandle` drops the shutdown sender, which unblocks
        // `run_with_shutdown` and stops accepting new connections.
        drop(self.server.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::{HeaderValue, IntoResponse, Json, Request, Response, StatusCode, header};
    use phoenix_routing::Routes;
    use serde_json::json;

    fn test_routes() -> Routes {
        Routes::new()
            .get("/health", |_request: Request| async {
                Json(json!({ "status": "healthy" }))
            })
            .get("/members", |request: Request| async move {
                members_page(&request)
            })
            .post("/login", |request: Request| async move {
                let mut response = Json(json!({ "ok": true })).into_response();
                response.headers_mut().append(
                    header::SET_COOKIE,
                    HeaderValue::from_static("session=abc; Path=/; HttpOnly"),
                );
                let _ = request;
                response
            })
            .get("/whoami", |request: Request| async move {
                let cookie = request
                    .headers()
                    .get(header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                Json(json!({ "cookie": cookie }))
            })
    }

    fn members_page(request: &Request) -> Response {
        let page_request = request
            .headers()
            .get("x-phoenix-page")
            .is_some_and(|value| value == "1");
        if page_request {
            let mut response = Response::new(
                StatusCode::OK,
                serde_json::to_vec(&json!({
                    "protocol": 1,
                    "page": "members",
                    "props": { "total": 2 },
                }))
                .expect("page JSON"),
            );
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.phoenix.page+json"),
            );
            response
        } else {
            Response::text("<html>members</html>")
        }
    }

    #[tokio::test]
    async fn get_json_asserts_ok_status_and_body() {
        let app = TestApp::spawn(test_routes()).await;
        app.get("/health")
            .send()
            .await
            .assert_ok()
            .assert_status(StatusCode::OK)
            .assert_body_contains("healthy")
            .assert_json_path("status", json!("healthy"))
            .assert_json(|value| {
                assert_eq!(value["status"], "healthy");
            });
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn missing_routes_return_404() {
        let app = TestApp::spawn(test_routes()).await;
        app.get("/missing")
            .send()
            .await
            .assert_status(StatusCode::NOT_FOUND);
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cookie_jar_round_trips_set_cookie() {
        let app = TestApp::spawn(test_routes()).await;
        app.post("/login").send().await.assert_ok();
        app.get("/whoami")
            .send()
            .await
            .assert_ok()
            .assert_json_path("cookie", json!("session=abc"));
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn page_protocol_asserts_page_name() {
        let app = TestApp::spawn(Application::new(test_routes())).await;
        app.get("/members")
            .page_protocol()
            .send()
            .await
            .assert_page("members");
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn post_json_and_custom_headers_work() {
        let routes = Routes::new().post("/echo", |request: Request| async move {
            let content_type = request
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let marker = request
                .headers()
                .get("x-test")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let body = String::from_utf8_lossy(request.body()).into_owned();
            Json(json!({ "content_type": content_type, "marker": marker, "body": body }))
        });
        let app = TestApp::spawn(routes).await;
        app.post_json("/echo", &json!({ "n": 1 }))
            .header("x-test", "yes")
            .send()
            .await
            .assert_ok()
            .assert_json(|value| {
                assert_eq!(value["content_type"], "application/json");
                assert_eq!(value["marker"], "yes");
                assert!(value["body"].as_str().unwrap().contains("\"n\":1"));
            });
        app.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn drop_stops_the_server() {
        let app = TestApp::spawn(test_routes()).await;
        let address = app.address();
        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let result = tokio::net::TcpStream::connect(address).await;
        assert!(result.is_err(), "server should stop accepting after Drop");
    }
}
