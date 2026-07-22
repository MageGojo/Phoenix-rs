pub use phoenix_core as core;
pub use phoenix_http as http;
pub use phoenix_routing as routing;
pub use phoenix_validation as validation;

pub mod prelude {
    pub use phoenix_core::{Application, Server, ServerError, ServerHandle};
    pub use phoenix_http::{
        BoxFuture, Handler, IntoResponse, Json, Method, Middleware, Next, Request, Response,
        StatusCode, Uri, middleware_fn,
    };
    pub use phoenix_routing::{RouteBuildError, RouteGroup, Router, Routes, UrlGenerationError};
    pub use phoenix_validation::{
        Rule, RuleContext, ValidationError, ValidationErrors, Validator, custom_rule, min_length,
        required, string,
    };
}
