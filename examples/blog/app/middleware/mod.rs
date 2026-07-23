use phoenix::{
    http::{BoxFuture, HeaderName, HeaderValue},
    prelude::{Middleware, Next, Request, Response, Session, StatusCode},
};

use crate::models::AuthStore;

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

/// Strongly typed identity of the signed-in user, injected by [`RequireAuth`].
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub role: String,
}

/// Require a session-backed login: reads `user_id` from the session, loads the
/// user and injects [`CurrentUser`] into the request extensions.
pub struct RequireAuth {
    store: AuthStore,
}

impl RequireAuth {
    #[must_use]
    pub const fn new(store: AuthStore) -> Self {
        Self { store }
    }
}

impl Middleware for RequireAuth {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let store = self.store.clone();
        Box::pin(async move {
            let user_id = request
                .extensions()
                .get::<Session>()
                .and_then(|session| session.get("user_id"))
                .and_then(|value| value.as_u64());
            let Some(user_id) = user_id else {
                return Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED);
            };
            match store.find_user(user_id).await {
                Ok(Some(user)) if !user.locked => {
                    request.extensions_mut().insert(CurrentUser {
                        id: user.id,
                        name: user.name,
                        email: user.email,
                        role: user.role,
                    });
                    next.run(request).await
                }
                Ok(_) => Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED),
                Err(error) => Response::text(format!("auth store error: {error}"))
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        })
    }
}
