pub use phoenix_core as core;
pub use phoenix_database as database;
pub use phoenix_dx as dx;
pub use phoenix_dx::mount_routes;
pub use phoenix_http as http;
pub use phoenix_routing as routing;
pub use phoenix_security as security;
pub use phoenix_validation as validation;
pub use phoenix_view as view;

pub mod prelude {
    pub use phoenix_core::{Application, Server, ServerError, ServerHandle};
    pub use phoenix_database::{Backend, Database, DatabaseBuilder, DatabaseError, TestDatabase};
    pub use phoenix_dx::{
        Bound, MiddlewareAliasError, MiddlewareAliases, ModelBinding, Resource, ResourceAction,
        ResourceRoutes, mount_routes,
    };
    pub use phoenix_http::{
        BoxFuture, ByteStream, Handler, IntoResponse, Json, JsonRejection, Method, Middleware,
        Next, Request, Response, ResponseBody, RouteManifest, SecurityHeaders, StatusCode, Uri,
        middleware_fn,
    };
    pub use phoenix_routing::{RouteBuildError, RouteGroup, Router, Routes, UrlGenerationError};
    pub use phoenix_security::{
        AccessLog, ClientIp, Cors, CorsConfig, Csrf, HostAllowlist, RateLimit, RateLimitConfig,
        RequestId, RequestIdValue, SameSite, SecurityPolicy, Session, SessionConfig,
        SessionMiddleware, SessionStore, TrustedProxies,
    };
    pub use phoenix_validation::{
        BoxedRule, Rule, RuleContext, ValidationError, ValidationErrors, Validator, custom_rule,
        min_length, required, rules, string,
    };
    pub use phoenix_view::{
        ASSET_MANIFEST_SCHEMA, Aes256GcmCodec, AssetEntry, AssetManifest, AssetManifestError,
        EncryptionError, Island, NodeRenderer, Page, PageEnvelope, PageResponseError, PayloadCodec,
        RenderContext, RenderFrame, RenderMode, RenderResult, RendererConfig, RendererError,
        RendererHealth, RendererManifest, RendererStream,
    };
}
