pub use phoenix_core as core;
pub use phoenix_core::applications;
pub use phoenix_crypto as crypto;
pub use phoenix_database as database;
pub use phoenix_dx as dx;
pub use phoenix_dx::mount_routes;
pub use phoenix_http as http;
pub use phoenix_logging as logging;
pub use phoenix_macros::contract;
pub use phoenix_routing as routing;
pub use phoenix_routing::routes;
pub use phoenix_security as security;
pub use phoenix_validation as validation;
pub use phoenix_view as view;

pub mod prelude {
    pub use phoenix_core::applications;
    pub use phoenix_core::{
        Application, ApplicationModule, HttpProtocol, MultiApplicationBuilder,
        MultiApplicationError, Server, ServerError, ServerHandle, TlsConfig, TlsConfigError,
    };
    pub use phoenix_crypto::{
        Ciphertext, EncryptionError as CryptoEncryptionError, EncryptionKey, Encryptor, Jwt,
        JwtAuth, JwtClaims, JwtConfig, JwtError, JwtKey, JwtManager, Password, PasswordError,
    };
    pub use phoenix_database::{Backend, Database, DatabaseBuilder, DatabaseError, TestDatabase};
    pub use phoenix_dx::{
        Bound, MiddlewareAliasError, MiddlewareAliases, ModelBinding, Resource, ResourceAction,
        ResourceRoutes, mount_routes,
    };
    pub use phoenix_http::{
        BoxFuture, ByteStream, ConnectionInfo, Download, Form, FormRejection, FromMultipart,
        FromRequest, Handler, Header, HeaderRejection, IntoResponse, Json, JsonRejection, Method,
        Middleware, Mime, Multipart, MultipartData, MultipartField, MultipartRejection, Next, Path,
        PathRejection, Query, QueryRejection, Redirect, Request, Response, ResponseBody,
        RouteManifest, SecurityHeaders, State, StateMiddleware, StateRejection, StatusCode,
        TransportScheme, TypedHandler, Uri, middleware_fn, typed,
    };
    pub use phoenix_logging::{LogFormat, Logging, LoggingError, LoggingGuard};
    pub use phoenix_routing::routes;
    pub use phoenix_routing::{
        ApplicationContext, MultiRouterError, RouteBuildError, RouteGroup, Router, Routes,
        UrlGenerationError,
    };
    pub use phoenix_security::{
        AccessLog, ClientIp, Cors, CorsConfig, Csrf, EffectiveScheme, HostAllowlist, HttpsRedirect,
        HttpsRedirectError, RateLimit, RateLimitConfig, RequestId, RequestIdValue, SameSite,
        SecurityPolicy, Session, SessionConfig, SessionMiddleware, SessionStore, TrustedProxies,
        effective_scheme,
    };
    pub use phoenix_validation::{
        BoxedRule, Rule, RuleContext, Validate, Validated, ValidatedRejection, ValidationError,
        ValidationErrors, Validator, custom_rule, max_length, min_length, required, rules, string,
    };
    pub use phoenix_view::{
        ASSET_MANIFEST_SCHEMA, Aes256GcmCodec, AssetEntry, AssetManifest, AssetManifestError,
        EncryptionError, Island, NodeRenderer, OpenGraph, Page, PageEnvelope, PageHead,
        PageResponseError, PayloadCodec, RenderContext, RenderFrame, RenderMode, RenderResult,
        RendererConfig, RendererError, RendererHealth, RendererManifest, RendererStream,
    };
}
