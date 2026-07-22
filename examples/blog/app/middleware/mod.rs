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

impl Middleware for RequireExampleToken {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        Box::pin(async move {
            let authorized = request
                .headers()
                .get("x-example-token")
                .is_some_and(|value| value == "secret");
            if authorized {
                next.run(request).await
            } else {
                Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED)
            }
        })
    }
}
