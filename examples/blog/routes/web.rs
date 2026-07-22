use phoenix::prelude::{RouteGroup, Routes, SecurityHeaders};

use crate::{
    controllers::{
        AdminController, HealthController, ReactController, RegistrationController, UserController,
    },
    middleware::{PoweredByPhoenix, RequireExampleToken},
};

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .with_middleware(SecurityHeaders)
        .with_middleware(PoweredByPhoenix)
        .get("/health", HealthController::show)
        .name("health")
        .get("/users/{user}", UserController::show)
        .name("users.show")
        .post("/register", RegistrationController::store)
        .name("register.store")
        .get("/react", ReactController::islands)
        .name("react.islands")
        .get("/react/spa", ReactController::spa)
        .name("react.spa")
        .get("/react/ssr", ReactController::ssr)
        .name("react.ssr")
        .group(
            RouteGroup::new()
                .prefix("/admin")
                .name("admin.")
                .middleware(RequireExampleToken),
            |routes| {
                routes
                    .get("/dashboard", AdminController::dashboard)
                    .name("dashboard")
            },
        )
}
