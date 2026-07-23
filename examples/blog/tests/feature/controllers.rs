use phoenix::{
    http::{Bytes, HeaderMap, HeaderValue, header},
    prelude::{Application, Method, Request, StatusCode, Uri},
};

#[tokio::test]
async fn registration_controller_returns_custom_validation_errors() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let request = csrf_json_request(
        &application,
        Method::POST,
        "/register",
        br#"{"user":"admin","password":"short"}"#,
    )
    .await;

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("response should be JSON");
    assert_eq!(json["errors"]["user"][0]["rule"], "not_reserved");
    assert_eq!(json["errors"]["password"][0]["rule"], "min_length");
}

#[tokio::test]
async fn registration_controller_accepts_valid_data() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let request = csrf_json_request(
        &application,
        Method::POST,
        "/register",
        br#"{"user":"phoenix-user","password":"correct-horse"}"#,
    )
    .await;

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn registration_controller_rejects_missing_content_type_and_invalid_json() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let missing_content_type = csrf_request(
        &application,
        Request::from_parts(
            Method::POST,
            Uri::from_static("/register"),
            HeaderMap::new(),
            Bytes::from_static(br#"{"user":"phoenix-user"}"#),
        ),
    )
    .await;
    let response = application.handle(missing_content_type).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    let wrong_content_type = csrf_request(
        &application,
        Request::from_parts(
            Method::POST,
            Uri::from_static("/register"),
            headers,
            Bytes::from_static(br#"{"user":"phoenix-user"}"#),
        ),
    )
    .await;
    let response = application.handle(wrong_content_type).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    let invalid_json = csrf_request(
        &application,
        Request::from_parts(
            Method::POST,
            Uri::from_static("/register"),
            headers,
            Bytes::from_static(br"{"),
        ),
    )
    .await;
    let response = application.handle(invalid_json).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("error should be JSON");
    assert_eq!(json["message"], "The request body contains invalid JSON.");
}

#[tokio::test]
async fn member_controller_creates_a_server_owned_member_from_input() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = csrf_request(
        &application,
        Request::from_parts(
            Method::POST,
            Uri::from_static("/api/members"),
            headers,
            Bytes::from(r#"{"name":"Rust 新成员"}"#),
        ),
    )
    .await;

    let response = application.handle(request).await;
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(body["name"], "Rust 新成员");
    assert_eq!(body["createdBy"], "Rust");
    assert_eq!(body["city"], "Rust 服务端");
    assert!(body["email"].as_str().unwrap().starts_with("rust"));
}

#[tokio::test]
async fn member_controller_rejects_an_empty_name() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let request = csrf_json_request(
        &application,
        Method::POST,
        "/api/members",
        br#"{"name":"  "}"#,
    )
    .await;

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("validation should be JSON");
    assert_eq!(body["errors"]["name"][0]["rule"], "required");
}

#[tokio::test]
async fn member_controller_maps_typed_json_rejections() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");
    let request = csrf_json_request(&application, Method::POST, "/api/members", br"{").await;

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("rejection should be JSON");
    assert_eq!(body["message"], "The request body contains invalid JSON.");
}

#[tokio::test]
async fn auth_flow_logs_in_logs_out_and_accepts_reset_requests() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");

    let (login, session_cookie) = csrf_session_json_request(
        &application,
        Method::POST,
        "/login",
        br#"{"email":"admin@example.test","password":"phoenix-password"}"#,
    )
    .await;
    let response = application.handle(login).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(body["subject"], "admin@example.test");
    assert_eq!(body["name"], "Ada Admin");
    assert_eq!(body["role"], "owner");

    let rejected = csrf_json_request(
        &application,
        Method::POST,
        "/login",
        br#"{"email":"admin@example.test","password":"wrong-password"}"#,
    )
    .await;
    let response = application.handle(rejected).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let reset = csrf_json_request(
        &application,
        Method::POST,
        "/password-reset",
        br#"{"email":"admin@example.test"}"#,
    )
    .await;
    let response = application.handle(reset).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut logout = Request::new(Method::POST, Uri::from_static("/logout"));
    logout
        .headers_mut()
        .insert(header::COOKIE, session_cookie.clone());
    let logout = csrf_request(&application, logout).await;
    let response = application.handle(logout).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(body["message"], "Signed out.");

    // After logout the session is destroyed: the old cookie no longer authorizes.
    let mut dashboard = Request::new(Method::GET, Uri::from_static("/admin/dashboard"));
    dashboard
        .headers_mut()
        .insert(header::COOKIE, session_cookie);
    let response = application.handle(dashboard).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutations_without_a_csrf_token_are_rejected() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");

    let login = json_request(
        Method::POST,
        "/login",
        br#"{"email":"admin@example.test","password":"phoenix-password"}"#,
    );
    let response = application.handle(login).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let logout = Request::new(Method::POST, Uri::from_static("/logout"));
    let response = application.handle(logout).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn locked_accounts_cannot_log_in() {
    let application = phoenix_blog_example::application()
        .await
        .expect("routes should build");

    let login = csrf_json_request(
        &application,
        Method::POST,
        "/login",
        br#"{"email":"operator@example.test","password":"phoenix-password"}"#,
    )
    .await;
    let response = application.handle(login).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Attach a fresh anonymous session cookie plus its CSRF token to `request`.
async fn csrf_request(application: &Application, request: Request) -> Request {
    csrf_session_request(application, request).await.0
}

async fn csrf_json_request(
    application: &Application,
    method: Method,
    uri: &'static str,
    body: &'static [u8],
) -> Request {
    csrf_request(application, json_request(method, uri, body)).await
}

/// Like [`csrf_request`] but also returns the session cookie so follow-up
/// requests can stay in the same session.
async fn csrf_session_json_request(
    application: &Application,
    method: Method,
    uri: &'static str,
    body: &'static [u8],
) -> (Request, HeaderValue) {
    csrf_session_request(application, json_request(method, uri, body)).await
}

async fn csrf_session_request(
    application: &Application,
    mut request: Request,
) -> (Request, HeaderValue) {
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
    request.headers_mut().insert(header::COOKIE, cookie.clone());
    request
        .headers_mut()
        .insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
    (request, cookie)
}

fn json_request(method: Method, uri: &'static str, body: &'static [u8]) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Request::from_parts(
        method,
        Uri::from_static(uri),
        headers,
        Bytes::from_static(body),
    )
}
