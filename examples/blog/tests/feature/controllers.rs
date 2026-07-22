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
