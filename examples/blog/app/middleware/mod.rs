use phoenix::{
    http::{BoxFuture, HeaderName, HeaderValue},
    prelude::{Middleware, Next, Request, Response, StatusCode},
};

pub struct PoweredByPhoenix;

impl Middleware for PoweredByPhoenix {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let mut response = next.run(request).await;
            response.headers_mut().insert(
                HeaderName::from_static("x-powered-by"),
                HeaderValue::from_static("Phoenix"),
            );
            response
        })
    }
}

pub struct RequireExampleToken;

#[derive(Clone, Copy, Debug)]
pub struct AuthorizedAdmin;

impl Middleware for RequireExampleToken {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let authorized = request
                .headers()
                .get("x-example-token")
                .is_some_and(|value| value == "secret");
            if authorized {
                request.extensions_mut().insert(AuthorizedAdmin);
                next.run(request).await
            } else {
                Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED)
            }
        })
    }
}
