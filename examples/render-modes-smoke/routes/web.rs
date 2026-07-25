use phoenix::prelude::Routes;

use crate::controllers::{HomeController, RenderModesController};

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .get("/", HomeController::index)
        .name("home")
        .get("/spa", RenderModesController::spa)
        .name("render.spa")
        .get("/islands", RenderModesController::islands)
        .name("render.islands")
        .get("/ssr", RenderModesController::ssr)
        .name("render.ssr")
}
