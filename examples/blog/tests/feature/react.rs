use phoenix::{
    http::{HeaderValue, Method, Request, Uri},
    prelude::{Aes256GcmCodec, NodeRenderer, Page, PageEnvelope, RendererConfig, StatusCode},
    view::EncryptedPayload,
};
use serde_json::json;

#[tokio::test]
async fn react_pages_default_to_islands_and_offer_spa_and_ssr() {
    let application = test_application();

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

#[tokio::test]
async fn member_directory_receives_one_hundred_unique_rust_records() {
    let application = phoenix_blog_example::application().expect("routes should build");
    let mut request = Request::new(Method::GET, Uri::from_static("/members"));
    request
        .headers_mut()
        .insert("x-phoenix-page", HeaderValue::from_static("1"));

    let response = application.handle(request).await;
    let envelope: PageEnvelope = serde_json::from_slice(response.body()).unwrap();
    let members = envelope.props["members"].as_array().unwrap();
    let names = members
        .iter()
        .map(|member| member["name"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(envelope.page, "members/index");
    assert_eq!(envelope.render_mode, phoenix::prelude::RenderMode::Islands);
    assert_eq!(envelope.islands.len(), 1);
    assert_eq!(envelope.islands[0].id, "member-directory");
    assert_eq!(envelope.islands[0].component, "member-directory");
    assert_eq!(
        envelope.islands[0].props["initialMembers"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    assert_eq!(members.len(), 100);
    assert_eq!(names.len(), 100);
    assert_eq!(envelope.props["generatedBy"], "Rust");
}

#[tokio::test]
async fn member_directory_islands_contains_server_html_and_hydration_root() {
    let application = test_application();
    let response = application
        .handle(Request::new(Method::GET, Uri::from_static("/members")))
        .await;
    let html = String::from_utf8_lossy(response.body());

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-phoenix-render-mode"),
        Some(&HeaderValue::from_static("islands"))
    );
    assert!(html.contains("团队成员目录"));
    assert!(html.contains("member001@example.test"));
    assert!(html.contains("data-phoenix-island=\"member-directory\""));
    assert!(html.contains("动态添加成员"));
    assert!(html.contains("views/members-islands-entry.tsx"));
    assert!(html.contains("id=\"phoenix-page\""));
}

fn test_application() -> phoenix::prelude::Application {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/renderer.mjs");
    phoenix_blog_example::application_with_renderer(&NodeRenderer::new(RendererConfig::node(
        fixture,
    )))
    .expect("routes should build")
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
