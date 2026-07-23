use std::time::Duration;

#[path = "../app/auth.rs"]
pub mod auth;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[path = "../routes/web.rs"]
#[allow(dead_code)]
mod web_routes;

use phoenix::prelude::{Application, NodeRenderer, RouteBuildError, Routes};

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes() -> Routes {
    phoenix::mount_routes!()
}

/// Build the example application and compile its routes.
///
/// # Errors
///
/// Returns a route build error when the example route table is invalid.
pub fn application() -> Result<Application, RouteBuildError> {
    configured_application(routes())
}

/// Build the example with an injected SSR renderer, primarily for isolated tests.
///
/// # Errors
///
/// Returns a route build error when the example route table is invalid.
pub fn application_with_renderer(renderer: &NodeRenderer) -> Result<Application, RouteBuildError> {
    configured_application(web_routes::routes_with_renderer(renderer))
}

fn configured_application(routes: Routes) -> Result<Application, RouteBuildError> {
    Application::new(routes).map(|application| {
        application
            .max_body_size(64 * 1024)
            .header_read_timeout(Duration::from_secs(5))
            .body_read_timeout(Duration::from_secs(10))
            .graceful_shutdown_timeout(Duration::from_secs(5))
    })
}
