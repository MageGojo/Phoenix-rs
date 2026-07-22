use phoenix::{
    http::{Bytes, HeaderMap},
    prelude::{Method, Request, StatusCode, Uri},
};

#[tokio::test]
async fn registration_controller_returns_custom_validation_errors() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        HeaderMap::new(),
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
    let request = Request::from_parts(
        Method::POST,
        Uri::from_static("/register"),
        HeaderMap::new(),
        Bytes::from_static(br#"{"user":"phoenix-user","password":"correct-horse"}"#),
    );

    let response = application.handle(request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
}
