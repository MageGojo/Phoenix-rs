use std::time::Duration;

#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../routes/web.rs"]
mod web_routes;

use phoenix::prelude::{Application, RouteBuildError, Routes};

#[must_use]
pub fn routes() -> Routes {
    web_routes::routes()
}

/// Build the example application and compile its routes.
///
/// # Errors
///
/// Returns a route build error when the example route table is invalid.
pub fn application() -> Result<Application, RouteBuildError> {
    Application::new(routes()).map(|application| {
        application
            .max_body_size(64 * 1024)
            .header_read_timeout(Duration::from_secs(5))
            .body_read_timeout(Duration::from_secs(10))
            .graceful_shutdown_timeout(Duration::from_secs(5))
    })
}
