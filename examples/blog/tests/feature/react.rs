use phoenix::{
    http::{HeaderValue, Method, Request, Uri},
    prelude::{Aes256GcmCodec, Page, PageEnvelope, StatusCode},
    view::EncryptedPayload,
};
use serde_json::json;

#[tokio::test]
async fn react_pages_default_to_islands_and_offer_spa_and_ssr() {
    let application = phoenix_blog_example::application().expect("routes should build");

    for (path, expected_mode) in [
        ("/react", "islands"),
        ("/react/spa", "spa"),
        ("/react/ssr", "ssr"),
    ] {
        let response = application
            .handle(Request::new(Method::GET, path.parse::<Uri>().unwrap()))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-phoenix-render-mode"),
            Some(&HeaderValue::from_static(expected_mode))
        );
        let html = String::from_utf8_lossy(response.body());
        assert!(html.contains("React meets Phoenix"));
        assert!(html.contains("phoenix-page"));
    }
}

#[tokio::test]
async fn client_navigation_receives_the_same_business_props() {
    let application = phoenix_blog_example::application().expect("routes should build");

    for path in ["/react", "/react/spa", "/react/ssr"] {
        let mut request = Request::new(Method::GET, path.parse::<Uri>().unwrap());
        request
            .headers_mut()
            .insert("x-phoenix-page", HeaderValue::from_static("1"));
        let response = application.handle(request).await;
        let envelope: PageEnvelope = serde_json::from_slice(response.body()).unwrap();

        assert_eq!(envelope.page, "articles/show");
        assert_eq!(envelope.props["title"], "React meets Phoenix");
    }
}

#[test]
fn optional_encryption_is_compatible_with_the_page_protocol() {
    let codec = Aes256GcmCodec::new("test-key", [42; 32]);
    let response = Page::new("account/show", json!({ "visibleToUser": true }))
        .respond(true, Some(&codec))
        .unwrap();
    let encrypted: EncryptedPayload = serde_json::from_slice(response.body()).unwrap();
    let plaintext = codec.decode(&encrypted).unwrap();
    let envelope: PageEnvelope = serde_json::from_slice(&plaintext).unwrap();

    assert_eq!(response.headers().get("x-phoenix-encrypted").unwrap(), "1");
    assert_eq!(envelope.props["visibleToUser"], true);
}
