#[path = "../app/commands/mod.rs"]
pub mod commands;
#[path = "../config/mod.rs"]
pub mod config;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/features/mod.rs"]
pub mod features;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[cfg(feature = "database")]
#[path = "../database/migrations/mod.rs"]
pub mod migrations;
#[cfg(feature = "database")]
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/plugins/mod.rs"]
pub mod plugins;
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
    AccessLog, Application, AssetManifest, Csrf, HostAllowlist, Metrics, MetricsMiddleware,
    NodeRenderer, NonceSecurityPolicy, RateLimit, RateLimitConfig, RendererConfig,
    RendererManifest, Request, RequestId, Routes, ServeProductionAssets, SessionConfig,
    SessionMiddleware, SessionStore, StateMiddleware, TrustedProxies,
};
#[cfg(feature = "database")]
use phoenix::prelude::{Database, DatabaseError};

use config::AppConfig;
use features::FeatureServices;

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes(
    config: &AppConfig,
    assets: Option<&AssetManifest>,
    renderer: &NodeRenderer,
    services: &FeatureServices,
    metrics: &Metrics,
) -> Routes {
    let session_config = SessionConfig {
        secure: config.public_url().starts_with("https://"),
        ..SessionConfig::default()
    };
    let session_store = SessionStore::memory(session_config.max_age);
    let metrics_endpoint = metrics.clone();
    let plugins = features::plugins().expect("greeter feature installs");

    let mut routes = phoenix::mount_routes!()
        .merge(features::http_routes(services))
        .merge(plugins.into_routes())
        .get("/internal/metrics", move |_request: Request| {
            let metrics = metrics_endpoint.clone();
            async move { metrics.response() }
        })
        .name("internal.metrics")
        .with_middleware(TrustedProxies::new(
            config.trusted_proxies().iter().copied(),
        ))
        .with_middleware(RequestId)
        .with_middleware(AccessLog)
        .with_middleware(MetricsMiddleware::new(metrics.clone()));
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
        .with_middleware(StateMiddleware::new(services.clone()))
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
    let services = FeatureServices::new()?;
    let metrics = Metrics::new();
    let built = routes(
        &config,
        assets.as_ref(),
        &renderer,
        &services,
        &metrics,
    )
    .with_middleware(StateMiddleware::new(db));
    Ok(Application::new(built)?.metrics(metrics))
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
