use phoenix::{
    http::{Bytes, HeaderMap, HeaderValue, header},
    prelude::{
        Method, Next, Request, Response, Routes, SecurityHeaders, StatusCode, Uri, middleware_fn,
    },
};

#[tokio::test]
async fn global_and_group_middleware_wrap_controller_responses() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");

    let response = application
        .handle(Request::new(
            Method::GET,
            Uri::from_static("/admin/dashboard"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("x-powered-by"),
        Some(&HeaderValue::from_static("Phoenix"))
    );
    assert_security_headers(&response);

    // Sign in through the session-backed login, then reuse the session cookie.
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
        .get(header::SET_COOKIE)
        .expect("SessionMiddleware should set a cookie")
        .clone();

    let mut login_headers = HeaderMap::new();
    login_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    login_headers.insert(header::COOKIE, cookie.clone());
    login_headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
    let login = Request::from_parts(
        Method::POST,
        Uri::from_static("/login"),
        login_headers,
        Bytes::from_static(br#"{"email":"admin@example.test","password":"phoenix-password"}"#),
    );
    let response = application.handle(login).await;
    assert_eq!(response.status(), StatusCode::OK);
    // Login rotates the session id; follow the newest cookie.
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .map_or(cookie, Clone::clone);

    let mut request = Request::new(Method::GET, Uri::from_static("/admin/dashboard"));
    request.headers_mut().insert(header::COOKIE, cookie);
    request
        .headers_mut()
        .insert("x-phoenix-page", HeaderValue::from_static("1"));
    let response = application.handle(request).await;

    assert_eq!(response.status(), StatusCode::OK);
    let envelope: phoenix::prelude::PageEnvelope =
        serde_json::from_slice(response.body()).expect("admin page envelope");
    assert_eq!(envelope.page, "admin/dashboard");
    assert_eq!(envelope.props["users"].as_array().unwrap().len(), 3);
    assert_eq!(envelope.shared["user"]["email"], "admin@example.test");
    assert_eq!(
        response.headers().get("x-powered-by"),
        Some(&HeaderValue::from_static("Phoenix"))
    );
    assert_security_headers(&response);
}

#[tokio::test]
async fn middleware_can_be_attached_to_one_route() {
    let router = Routes::new()
        .get("/guarded", |_request| std::future::ready("guarded"))
        .middleware(middleware_fn(|request: Request, next: Next| async move {
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .insert("x-route-middleware", HeaderValue::from_static("applied"));
            response
        }))
        .get("/plain", |_request| std::future::ready("plain"))
        .build()
        .expect("routes should build");

    let guarded = router
        .handle(Request::new(Method::GET, Uri::from_static("/guarded")))
        .await;
    assert_eq!(
        guarded.headers().get("x-route-middleware"),
        Some(&HeaderValue::from_static("applied"))
    );

    let plain = router
        .handle(Request::new(Method::GET, Uri::from_static("/plain")))
        .await;
    assert!(plain.headers().get("x-route-middleware").is_none());
}

fn assert_security_headers(response: &Response) {
    assert_eq!(
        response.headers().get("x-content-type-options"),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        response.headers().get("x-frame-options"),
        Some(&HeaderValue::from_static("DENY"))
    );
    assert_eq!(
        response.headers().get("referrer-policy"),
        Some(&HeaderValue::from_static("strict-origin-when-cross-origin"))
    );
}

#[tokio::test]
async fn security_headers_do_not_override_explicit_application_policy() {
    let router = Routes::new()
        .with_middleware(SecurityHeaders)
        .get("/embedded", |_request| {
            std::future::ready(
                Response::text("embedded")
                    .with_header("x-frame-options", "SAMEORIGIN")
                    .expect("static header should be valid"),
            )
        })
        .build()
        .expect("routes should build");

    let response = router
        .handle(Request::new(Method::GET, Uri::from_static("/embedded")))
        .await;
    assert_eq!(
        response.headers().get("x-frame-options"),
        Some(&HeaderValue::from_static("SAMEORIGIN"))
    );
}
