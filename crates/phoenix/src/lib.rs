pub use phoenix_auth as auth;
pub use phoenix_config as config;
pub use phoenix_console as console;
pub use phoenix_console::commands;
pub use phoenix_runtime as core;
pub use phoenix_runtime::applications;
pub use phoenix_crypto as crypto;
pub use phoenix_database as database;
pub use phoenix_dx as dx;
pub use phoenix_dx::mount_routes;
pub use phoenix_http as http;
pub use phoenix_logging as logging;
pub use phoenix_macros::contract;
pub use phoenix_metrics as metrics;
pub use phoenix_plugin as plugin;
pub use phoenix_routing as routing;
pub use phoenix_routing::routes;
pub use phoenix_security as security;
pub use phoenix_validation as validation;
pub use phoenix_view as view;

#[cfg(feature = "mail")]
pub use phoenix_mail as mail;
#[cfg(feature = "queue")]
pub use phoenix_queue as queue;
#[cfg(feature = "redis")]
pub use phoenix_redis as redis;
#[cfg(feature = "storage")]
pub use phoenix_storage as storage;
#[cfg(feature = "testing")]
pub use phoenix_testing as testing;

pub mod prelude {
    pub use phoenix_auth::{
        AbacPolicy, AuditReason, AuthorizationAudit, AuthorizationAuditEvent,
        AuthorizationDecision, AuthorizationEngine, AuthorizationError, AuthorizationRequest,
        CurrentPrincipal, Permission, PolicyFn, Principal, PrincipalFromJwt, Rbac, RbacError,
        RequirePermission, Role, policy_fn,
    };
    pub use phoenix_config::{AppConfig, AppConfigBuilder, ConfigError, Environment, SecretValue};
    pub use phoenix_console::{CommandContext, CommandEntry, CommandResult, Console, commands};
    pub use phoenix_runtime::applications;
    pub use phoenix_runtime::{
        Application, ApplicationModule, HttpProtocol, MultiApplicationBuilder,
        MultiApplicationError, Server, ServerError, ServerHandle, TlsConfig, TlsConfigError,
    };
    pub use phoenix_crypto::{
        BlindIndexError, BlindIndexKey, BlindIndexer, Ciphertext,
        EncryptionError as CryptoEncryptionError, EncryptionKey, Encryptor, FileTokenStore, Jwt,
        JwtAuth, JwtClaims, JwtConfig, JwtError, JwtKey, JwtManager, MAX_BLIND_INDEX_KEYS,
        MemoryTokenStore, Password, PasswordError, RefreshRecord, RotateRefresh, StatefulJwtAuth,
        TokenError, TokenPair, TokenService, TokenStore, TokenStoreError,
    };
    pub use phoenix_database::{Backend, Database, DatabaseBuilder, DatabaseError, TestDatabase};
    pub use phoenix_dx::{
        Bound, MiddlewareAliasError, MiddlewareAliases, ModelBinding, Resource, ResourceAction,
        ResourceRoutes, mount_routes,
    };
    pub use phoenix_http::{
        BoxFuture, ByteStream, CloseCode, CloseFrame, ConnectionInfo, CspNonce, Download, Form,
        FormRejection, FromMultipart, FromRequest, Handler, Header, HeaderRejection, IntoResponse,
        InvalidCspNonce, InvalidSseField, Json, JsonRejection, KeepAlive, LastEventId, Message,
        Method, Middleware, Mime, Multipart, MultipartData, MultipartField, MultipartRejection,
        Next, Path, PathRejection, Query, QueryRejection, Redirect, Request, RequestBodyError,
        RequestBodyMode, RequestBodyStream, RequestBodyStreamRejection, Response, ResponseBody,
        ResponseContext, RouteManifest, SecurityHeaders, Sse, SseConfigError, SseEvent, State,
        StateMiddleware, StateRejection, StatusCode, StreamingHandler, TransportScheme,
        TypedHandler, Uri, Version, WebSocket, WebSocketConfigError, WebSocketError,
        WebSocketUpgrade, WebSocketUpgradeRejection, middleware_fn, streaming, typed,
    };
    pub use phoenix_logging::{LogFormat, Logging, LoggingError, LoggingGuard};
    pub use phoenix_metrics::{
        DatabaseOutcome, JobOutcome, Metrics, MetricsMiddleware, RendererMetricsSnapshot,
    };
    pub use phoenix_plugin::{Capability, FeatureError, FeatureParts, FeatureSet, Plugin};
    pub use phoenix_routing::routes;
    pub use phoenix_routing::{
        ApplicationContext, MultiRouterError, RouteBuildError, RouteGroup, Router, Routes,
        UrlGenerationError,
    };
    pub use phoenix_security::{
        AccessLog, ClientIp, ClientIpRateLimitKey, Cors, CorsConfig, CspPolicyError, Csrf,
        EffectiveScheme, HostAllowlist, HttpsRedirect, HttpsRedirectError, MemoryRateLimitBackend,
        MemorySessionBackend, NonceSecurityPolicy, RateLimit, RateLimitBackend, RateLimitConfig,
        RateLimitDecision, RateLimitFailureMode, RateLimitKey, RateLimitStoreError, RequestId,
        RequestIdValue, SameSite, SecurityPolicy, Session, SessionBackend, SessionBackendError,
        SessionConfig, SessionMiddleware, SessionSnapshot, SessionStore, SessionWrite,
        TrustedProxies, effective_scheme,
    };
    pub use phoenix_validation::{
        BoxedRule, Rule, RuleContext, Validate, Validated, ValidatedRejection, ValidationError,
        ValidationErrors, Validator, custom_rule, max_length, min_length, required, rules, string,
    };
    pub use phoenix_view::{
        ASSET_MANIFEST_SCHEMA, Aes256GcmCodec, AssetEntry, AssetManifest, AssetManifestError,
        DocumentContext, DocumentSlots, DocumentTemplate, DocumentTemplateError, EncryptionError,
        Island, NodeRenderer, OpenGraph, Page, PageEnvelope, PageHead, PageResponseError,
        PayloadCodec, RenderContext, RenderFrame, RenderMode, RenderResult, RendererConfig,
        RendererError, RendererHealth, RendererManifest, RendererStream, TrustedHtml,
    };

    #[cfg(feature = "mail")]
    pub use phoenix_mail::{
        Address, MailError, MailTransport, Mailer, MemoryTransport, Message as EmailMessage,
        MessageBuilder,
    };
    #[cfg(feature = "queue")]
    pub use phoenix_queue::{
        JobEnvelope, JobError, JobHandler, JobId, MemoryQueue, PushOptions, PushResult, Queue,
        QueueBackend, QueueError, ShutdownSignal, ShutdownToken, Worker, WorkerConfig,
    };
    #[cfg(feature = "redis")]
    pub use phoenix_redis::{
        RedisBackends, RedisConnectError, RedisRateLimitBackend, RedisSessionBackend, RedisStores,
        RedisTokenStore,
    };
    #[cfg(feature = "storage")]
    pub use phoenix_storage::{LocalDisk, Storage, StorageError, sanitize_key};
    #[cfg(feature = "testing")]
    pub use phoenix_testing::{
        IntoApplication, RequestBuilder, TestApp, TestAppError, TestResponse,
    };
}
