use std::{path::PathBuf, time::Duration};

use phoenix::prelude::{
    NodeRenderer, NonceSecurityPolicy, RendererConfig, RouteGroup, Routes, typed,
};

use crate::{
    controllers::{
        AdminController, HealthController, MemberController, ReactController,
        RegistrationController, UserController,
    },
    middleware::{PoweredByPhoenix, RequireExampleToken},
    requests::StoreMemberInput,
    resources::MemberResource,
};

#[must_use]
pub fn routes() -> Routes {
    routes_with_renderer(&ssr_renderer())
}

#[must_use]
pub fn routes_with_renderer(renderer: &NodeRenderer) -> Routes {
    let article_renderer = renderer.clone();
    let article_islands_renderer = renderer.clone();
    let member_renderer = renderer.clone();

    Routes::new()
        .with_middleware(security_policy())
        .with_middleware(PoweredByPhoenix)
        .get("/health", HealthController::show)
        .name("health")
        .get("/users/{user}", UserController::show)
        .name("users.show")
        .post("/register", RegistrationController::store)
        .name("register.store")
        .post("/api/members", typed(MemberController::store))
        .name("members.store")
        .action::<StoreMemberInput, MemberResource>()
        .get("/react", move |request| {
            ReactController::islands(request, article_islands_renderer.clone())
        })
        .name("react.islands")
        .get("/react/spa", ReactController::spa)
        .name("react.spa")
        .get("/react/ssr", move |request| {
            ReactController::ssr(request, article_renderer.clone())
        })
        .name("react.ssr")
        .get("/members", move |request| {
            ReactController::members(request, member_renderer.clone())
        })
        .name("members.index")
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

fn security_policy() -> NonceSecurityPolicy {
    if cfg!(debug_assertions) {
        let vite_origin =
            std::env::var("VITE_DEV_URL").unwrap_or_else(|_| "http://127.0.0.1:5173".to_owned());
        return NonceSecurityPolicy::development(&vite_origin)
            .expect("VITE_DEV_URL must be one trusted HTTP(S) origin");
    }
    NonceSecurityPolicy::default()
}

fn ssr_renderer() -> NodeRenderer {
    let entrypoint = std::env::var_os("PHOENIX_SSR_ENTRY").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public/ssr/renderer.js"),
        PathBuf::from,
    );
    NodeRenderer::new(RendererConfig::node(entrypoint).with_timeout(Duration::from_secs(2)))
}
