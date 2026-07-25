#[path = "../app/commands/mod.rs"]
pub mod commands;
#[path = "../config/mod.rs"]
pub mod config;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[cfg(feature = "database")]
#[path = "../database/migrations/mod.rs"]
pub mod migrations;
#[cfg(feature = "database")]
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[cfg(feature = "database")]
#[path = "../database/seeders/mod.rs"]
pub mod seeders;

use phoenix::prelude::{
    AccessLog, Application, AssetManifest, Csrf, HostAllowlist, NodeRenderer, NonceSecurityPolicy,
    RateLimit, RateLimitConfig, RendererConfig, RendererManifest, RequestId, Routes,
    ServeProductionAssets, SessionConfig, SessionMiddleware, SessionStore, StateMiddleware,
    TrustedProxies,
};
#[cfg(feature = "database")]
use phoenix::prelude::{Database, DatabaseError};

use config::AppConfig;

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes(
    config: &AppConfig,
    assets: Option<&AssetManifest>,
    renderer: &NodeRenderer,
) -> Routes {
    let session_config = SessionConfig {
        secure: config.public_url().starts_with("https://"),
        ..SessionConfig::default()
    };
    let session_store = SessionStore::memory(session_config.max_age);

    let mut routes = phoenix::mount_routes!()
        .with_middleware(TrustedProxies::new(
            config.trusted_proxies().iter().copied(),
        ))
        .with_middleware(RequestId)
        .with_middleware(AccessLog);
    if let Some(assets) = assets.cloned() {
        // Serve hashed Vite assets before session/CSRF so static GETs stay cheap.
        routes = routes.with_middleware(ServeProductionAssets::new(assets, "public/assets"));
    }
    routes
        .with_middleware(HostAllowlist::new(config.allowed_hosts().iter().cloned()))
        .with_middleware(RateLimit::new(RateLimitConfig {
            requests: config.rate_limit_requests(),
            window: config.rate_limit_window(),
        }))
        .with_middleware(content_security_policy(config))
        .with_middleware(SessionMiddleware::new(session_store, session_config))
        .with_middleware(Csrf)
        .with_middleware(StateMiddleware::new(config.clone()))
        .with_middleware(StateMiddleware::new(assets.cloned()))
        .with_middleware(StateMiddleware::new(renderer.clone()))
}

fn content_security_policy(config: &AppConfig) -> NonceSecurityPolicy {
    if !config.environment().is_production() {
        return NonceSecurityPolicy::development(
            config
                .vite_dev_url()
                .expect("development configuration always has a Vite origin"),
        )
        .expect("AppConfig validates VITE_DEV_URL as one trusted HTTP(S) origin");
    }
    NonceSecurityPolicy::default()
}

/// Build the Phoenix application.
///
/// # Errors
///
/// Returns a route error when route names or patterns conflict.
pub async fn application(
    config: AppConfig,
) -> Result<Application, Box<dyn std::error::Error + Send + Sync>> {
    let vite_dev_server = std::env::var_os("PHOENIX_VITE_DEV").is_some();
    let (assets, renderer) = if !vite_dev_server {
        let assets = AssetManifest::load("public/assets/phoenix-manifest.json")?;
        let renderer_manifest = RendererManifest::load("public/ssr/phoenix-renderer.json")?;
        let renderer = NodeRenderer::new(RendererConfig::production(
            &assets,
            &renderer_manifest,
            "public/ssr",
        )?);
        (Some(assets), renderer)
    } else {
        // `px dev` sets PHOENIX_VITE_DEV so this process uses Vite's browser
        // entry while HMR/full reload remains live.
        (
            None,
            NodeRenderer::new(RendererConfig::node("public/ssr/renderer.js")),
        )
    };
    renderer.warm_up().await?;
    let db = database(&config).await?;
    Ok(Application::new(
        routes(&config, assets.as_ref(), &renderer)
            .with_middleware(StateMiddleware::new(db)),
    )?)
}

/// Connect the configured database with every registered Toasty model.
///
/// # Errors
///
/// Returns a database error when the URL or connection is invalid.
#[cfg(feature = "database")]
pub async fn database(config: &AppConfig) -> Result<Database, DatabaseError> {
    Database::builder(models::all())
        .connect(config.database_url())
        .await
}
