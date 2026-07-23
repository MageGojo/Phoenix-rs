use std::{path::PathBuf, time::Duration};

use phoenix::prelude::{
    NodeRenderer, NonceSecurityPolicy, RendererConfig, RouteGroup, Routes, typed,
};

use crate::{
    controllers::{
        AdminController, AuthController, HealthController, MemberController, ReactController,
        RegistrationController, UserController,
    },
    middleware::{PoweredByPhoenix, RequireAuth},
    models::AuthStore,
    requests::{LoginInput, PasswordResetInput, StoreMemberInput},
    resources::{AuthMessageResource, AuthSessionResource, MemberResource},
};

#[must_use]
pub fn routes() -> Routes {
    let store = crate::models::auth_store()
        .expect("auth store should be installed before routes are mounted");
    routes_with_renderer(&ssr_renderer(), &store)
}

/// Routes bound to an explicit store, bypassing the process-wide registry.
#[must_use]
#[allow(dead_code)] // used via `web_routes_with_store`; module-level allow does not propagate
pub fn routes_with_store(store: &AuthStore) -> Routes {
    routes_with_renderer(&ssr_renderer(), store)
}

#[must_use]
pub fn routes_with_renderer(renderer: &NodeRenderer, store: &AuthStore) -> Routes {
    let article_renderer = renderer.clone();
    let article_islands_renderer = renderer.clone();
    let member_renderer = renderer.clone();
    let admin_renderer = renderer.clone();
    let login_store = store.clone();
    let dashboard_store = store.clone();

    Routes::new()
        .with_middleware(security_policy())
        .with_middleware(PoweredByPhoenix)
        .get("/health", HealthController::show)
        .name("health")
        .get("/users/{user}", UserController::show)
        .name("users.show")
        .post("/register", RegistrationController::store)
        .name("register.store")
        .post("/login", move |request| {
            AuthController::login(request, login_store.clone())
        })
        .name("login.store")
        .action::<LoginInput, AuthSessionResource>()
        .post("/logout", AuthController::logout)
        .name("logout.store")
        .post(
            "/password-reset",
            typed(AuthController::request_password_reset),
        )
        .name("password-reset.store")
        .action::<PasswordResetInput, AuthMessageResource>()
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
                .middleware(RequireAuth::new(store.clone())),
            |routes| {
                routes
                    .get("/dashboard", move |request| {
                        AdminController::dashboard(
                            request,
                            admin_renderer.clone(),
                            dashboard_store.clone(),
                        )
                    })
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
