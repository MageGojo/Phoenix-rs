use phoenix::prelude::{RouteGroup, Routes};

use crate::{
    controllers::{AdminController, HealthController, RegistrationController, UserController},
    middleware::{PoweredByPhoenix, RequireExampleToken},
};

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .with_middleware(PoweredByPhoenix)
        .get("/health", HealthController::show)
        .name("health")
        .get("/users/{user}", UserController::show)
        .name("users.show")
        .post("/register", RegistrationController::store)
        .name("register.store")
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
