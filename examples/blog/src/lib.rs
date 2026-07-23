use std::time::Duration;

#[path = "../app/auth.rs"]
pub mod auth;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[path = "../routes/web.rs"]
#[allow(dead_code)]
mod web_routes;

use phoenix::prelude::{
    Application, Csrf, NodeRenderer, RouteBuildError, Routes, SessionConfig, SessionMiddleware,
    SessionStore, StateMiddleware,
};

use crate::models::AuthStore;

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes() -> Routes {
    phoenix::mount_routes!()
}

/// Routes bound to an explicit store (no process-wide registry needed).
#[must_use]
pub fn web_routes_with_store(store: &AuthStore) -> Routes {
    web_routes::routes_with_store(store)
}

/// Build the example application and compile its routes.
///
/// # Errors
///
/// Returns a route build error when the example route table is invalid.
pub async fn application() -> Result<Application, RouteBuildError> {
    let store = demo_store().await;
    let routes = web_routes::routes_with_store(&store);
    configured_application(routes, store)
}

/// Build the example with an injected SSR renderer, primarily for isolated tests.
///
/// # Errors
///
/// Returns a route build error when the example route table is invalid.
pub async fn application_with_renderer(
    renderer: &NodeRenderer,
) -> Result<Application, RouteBuildError> {
    let store = demo_store().await;
    let routes = web_routes::routes_with_renderer(renderer, &store);
    configured_application(routes, store)
}

async fn demo_store() -> AuthStore {
    let store = AuthStore::in_memory()
        .await
        .expect("in-memory auth store should build");
    store
        .seed_demo_users()
        .await
        .expect("demo users should seed");
    store
}

fn configured_application(
    routes: Routes,
    store: AuthStore,
) -> Result<Application, RouteBuildError> {
    let session_config = SessionConfig::default();
    let session_store = SessionStore::memory(session_config.max_age);
    Application::new(
        routes
            .with_middleware(SessionMiddleware::new(session_store, session_config))
            .with_middleware(Csrf)
            .with_middleware(StateMiddleware::new(store)),
    )
    .map(|application| {
        application
            .max_body_size(64 * 1024)
            .header_read_timeout(Duration::from_secs(5))
            .body_read_timeout(Duration::from_secs(10))
            .graceful_shutdown_timeout(Duration::from_secs(5))
    })
}
