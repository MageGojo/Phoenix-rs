use std::future::{Ready, ready};

use phoenix::prelude::{Method, Request, Routes, SecurityHeaders, StatusCode, Uri};

#[tokio::test]
async fn route_parameters_and_http_methods_are_dispatched() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");

    let response = application
        .handle(Request::new(
            Method::GET,
            Uri::from_static("/users/Ada%20Lovelace"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("controller should return JSON");
    assert_eq!(json["user"], "Ada Lovelace");
    assert_eq!(json["route"], "users.show");

    // CSRF fails closed before the router sees the disallowed method.
    let response = application
        .handle(Request::new(Method::POST, Uri::from_static("/users/42")))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // A valid CSRF token lets the request reach the router: 405 + Allow.
    let response = application
        .handle(
            csrf(
                &application,
                Request::new(Method::POST, Uri::from_static("/users/42")),
            )
            .await,
        )
        .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("GET, HEAD")
    );

    let response = application
        .handle(Request::new(Method::GET, Uri::from_static("/missing")))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_path_parameters_are_rejected_without_lossy_decoding() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    for uri in ["/users/%FF", "/users/%ZZ"] {
        let response = application
            .handle(Request::new(
                Method::GET,
                uri.parse().expect("test URI should parse"),
            ))
            .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.body(), "Invalid path parameter encoding");
    }
}

#[tokio::test]
async fn handler_panics_are_isolated_and_do_not_expose_details() {
    fn panic_handler(_request: Request) -> Ready<&'static str> {
        panic!("private database details");
    }

    fn safe_handler(_request: Request) -> Ready<&'static str> {
        ready("still healthy")
    }

    let router = Routes::new()
        .with_middleware(SecurityHeaders)
        .get("/panic", panic_handler)
        .get("/safe", safe_handler)
        .build()
        .expect("routes should build");

    let response = router
        .handle(Request::new(Method::GET, Uri::from_static("/panic")))
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.body(), "Internal Server Error");
    assert!(!String::from_utf8_lossy(response.body()).contains("database"));
    assert_eq!(
        response.headers().get("x-content-type-options"),
        Some(&phoenix::http::HeaderValue::from_static("nosniff"))
    );

    let response = router
        .handle(Request::new(Method::GET, Uri::from_static("/safe")))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), "still healthy");
}

#[tokio::test]
async fn common_http_methods_head_and_options_are_supported() {
    fn handler(_request: Request) -> Ready<&'static str> {
        ready("ok")
    }

    let router = Routes::new()
        .get("/head", handler)
        .put("/resource", handler)
        .patch("/resource", handler)
        .delete("/resource", handler)
        .build()
        .expect("routes should build");

    for method in [Method::PUT, Method::PATCH, Method::DELETE] {
        let response = router
            .handle(Request::new(method, Uri::from_static("/resource")))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "ok");
    }

    let response = router
        .handle(Request::new(Method::HEAD, Uri::from_static("/head")))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.body().is_empty());

    let response = router
        .handle(Request::new(Method::OPTIONS, Uri::from_static("/resource")))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("DELETE, PATCH, PUT")
    );
}

/// Attach a session cookie + CSRF token so global middleware lets the
/// request reach the router.
async fn csrf(application: &phoenix::prelude::Application, mut request: Request) -> Request {
    let probe = application
        .handle(Request::new(Method::GET, Uri::from_static("/health")))
        .await;
    let token = probe
        .headers()
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .expect("Csrf middleware should emit a token");
    let cookie = probe
        .headers()
        .get(phoenix::http::header::SET_COOKIE)
        .expect("SessionMiddleware should set a cookie")
        .clone();
    request
        .headers_mut()
        .insert(phoenix::http::header::COOKIE, cookie);
    request.headers_mut().insert(
        "x-csrf-token",
        phoenix::http::HeaderValue::from_str(&token).unwrap(),
    );
    request
}
