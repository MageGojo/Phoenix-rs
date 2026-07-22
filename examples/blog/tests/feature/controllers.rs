use phoenix::{
    http::{Bytes, HeaderMap, HeaderValue, header},
    prelude::{Method, Request, StatusCode, Uri},
};

#[tokio::test]
async fn registration_controller_returns_custom_validation_errors() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        headers,
        Bytes::from_static(br#"{"user":"admin","password":"short"}"#),
    );

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("response should be JSON");
    assert_eq!(json["errors"]["user"][0]["rule"], "not_reserved");
    assert_eq!(json["errors"]["password"][0]["rule"], "min_length");
}

#[tokio::test]
async fn registration_controller_accepts_valid_data() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.phoenix+json"),
    );
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        headers,
        Bytes::from_static(br#"{"user":"phoenix-user","password":"correct-horse"}"#),
    );

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn registration_controller_rejects_missing_content_type_and_invalid_json() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let missing_content_type = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        HeaderMap::new(),
        Bytes::from_static(br#"{"user":"phoenix-user"}"#),
    );
    let response = application.handle(missing_content_type).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    let wrong_content_type = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        headers,
        Bytes::from_static(br#"{"user":"phoenix-user"}"#),
    );
    let response = application.handle(wrong_content_type).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    let invalid_json = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        headers,
        Bytes::from_static(br"{"),
    );
    let response = application.handle(invalid_json).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value =
        serde_json::from_slice(response.body()).expect("error should be JSON");
    assert_eq!(json["message"], "The request body contains invalid JSON.");
}

#[tokio::test]
async fn member_controller_creates_a_server_owned_member_from_input() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/api/members"),
        headers,
        Bytes::from(r#"{"name":"Rust 新成员"}"#),
    );

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
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/api/members"),
        headers,
        Bytes::from_static(br#"{"name":"  "}"#),
    );

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("validation should be JSON");
    assert_eq!(body["errors"]["name"][0]["rule"], "required");
}

#[tokio::test]
async fn member_controller_maps_typed_json_rejections() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/api/members"),
        headers,
        Bytes::from_static(br"{"),
    );

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("rejection should be JSON");
    assert_eq!(body["message"], "The request body contains invalid JSON.");
}
