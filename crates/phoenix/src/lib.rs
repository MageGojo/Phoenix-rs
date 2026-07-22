pub use phoenix_core as core;
pub use phoenix_http as http;
pub use phoenix_routing as routing;
pub use phoenix_validation as validation;
pub use phoenix_view as view;

pub mod prelude {
    pub use phoenix_core::{Application, Server, ServerError, ServerHandle};
    pub use phoenix_http::{
        BoxFuture, Handler, IntoResponse, Json, JsonRejection, Method, Middleware, Next, Request,
        Response, RouteManifest, SecurityHeaders, StatusCode, Uri, middleware_fn,
    };
    pub use phoenix_routing::{RouteBuildError, RouteGroup, Router, Routes, UrlGenerationError};
    pub use phoenix_validation::{
        BoxedRule, Rule, RuleContext, ValidationError, ValidationErrors, Validator, custom_rule,
        min_length, required, rules, string,
    };
    pub use phoenix_view::{
        Aes256GcmCodec, EncryptionError, Island, NodeRenderer, Page, PageEnvelope,
        PageResponseError, PayloadCodec, RenderContext, RenderMode, RenderResult, RendererConfig,
        RendererError,
    };
}
