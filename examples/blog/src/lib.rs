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
    Application::new(routes())
}
