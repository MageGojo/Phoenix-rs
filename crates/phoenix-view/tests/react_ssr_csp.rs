use futures_util::StreamExt;
use phoenix_http::{Method, Request, ResponseBody, StatusCode, header};
use phoenix_routing::Routes;
use phoenix_security::NonceSecurityPolicy;
use phoenix_view::{NodeRenderer, Page, RendererConfig};
use serde_json::json;

#[tokio::test]
#[ignore = "run through `npm run test:e2e:ssr-csp` after building the official renderer package"]
async fn official_react_renderer_propagates_one_nonce_through_suspense_and_html() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/official-react-ssr-suspense.mjs");
    let renderer = NodeRenderer::new(RendererConfig::node(fixture));
    renderer
        .warm_up()
        .await
        .expect("official renderer protocol handshake");
    let route_renderer = renderer.clone();
    let router = Routes::new()
        .get("/", move |request: Request| {
            let renderer = route_renderer.clone();
            async move {
                Page::new("tests/suspense", json!({ "ready": true }))
                    .ssr()
                    .respond_streaming_with_renderer(&request, &renderer)
            }
        })
        .with_middleware(NonceSecurityPolicy::default())
        .build()
        .unwrap();

    let response = router
        .handle(Request::new(Method::GET, "/".parse().unwrap()))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let policy = response.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    let nonce = policy
        .split_once("'nonce-")
        .and_then(|(_, source)| source.split_once('\''))
        .map(|(nonce, _)| nonce.to_owned())
        .expect("request nonce in CSP");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let (_, _, body) = response.into_parts();
    let ResponseBody::Stream(stream) = body else {
        panic!("expected a streaming response");
    };
    let html = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .fold(Vec::new(), |mut output, chunk| {
            output.extend_from_slice(&chunk);
            output
        });
    let html = String::from_utf8(html).unwrap();

    assert!(html.contains("id=\"fallback\""));
    assert!(html.contains("id=\"resolved\""));
    assert!(html.contains(&format!("<meta property=\"csp-nonce\" nonce=\"{nonce}\">")));
    let hydration_index = html.find("id=\"phoenix-page\"").unwrap();
    let resolved_index = html.find("id=\"resolved\"").unwrap();
    assert!(resolved_index < hydration_index);
    let recovery = &html[resolved_index..hydration_index];
    assert!(recovery.contains("<script"));
    assert!(recovery.contains(&format!("nonce=\"{nonce}\"")));

    let mut script_count = 0;
    let mut remaining = html.as_str();
    while let Some(start) = remaining.find("<script") {
        let script = &remaining[start..];
        let (tag, content) = script.split_once('>').expect("complete script start tag");
        assert!(
            tag.contains(&format!("nonce=\"{nonce}\"")),
            "script tag did not use the response nonce: {tag}"
        );
        let (_, after) = content
            .split_once("</script>")
            .expect("complete script element");
        remaining = after;
        script_count += 1;
    }
    assert!(script_count >= 3, "expected React and Phoenix script tags");
    assert!(!html.contains("\"csp_nonce\""));
    assert_eq!(renderer.health().rendered_requests, 1);
    renderer.shutdown().await;
}
