//! Cross-process contract for [`RedisSecureSessionStore`], gated by
//! `PHOENIX_TEST_REDIS_URL`.
//!
//! Two `SecureTransport` instances sharing one Redis stand in for two server
//! processes behind a load balancer. The property under test is the whole
//! reason the store exists: a client that handshakes with instance A can be
//! routed to instance B on its next request and still get an encrypted reply.
//!
//! The in-process store is checked against the same script as a control — it
//! must *not* share, which is exactly why sticky routing is required without
//! this store.

use std::sync::Arc;
use std::time::Duration;

use phoenix_crypto::{
    MemorySecureSessionStore, SecureSessionStore, SecureTransport, SecureTransportConfig,
};
use phoenix_http::{
    HeaderMap, HeaderValue, Method, PAGE_PROTOCOL_MEDIA_TYPE, Request, Response,
    SECURE_CONTENT_TYPE, SECURE_ENCRYPTED_HEADER, SECURE_KEY_HEADER, SECURE_REQUEST_HEADER,
    StatusCode, Uri, header,
};
use phoenix_redis::RedisStores;
use phoenix_routing::{Router, Routes};

fn redis_url() -> Option<String> {
    std::env::var("PHOENIX_TEST_REDIS_URL")
        .ok()
        .filter(|value| !value.is_empty())
}

async fn redis_store() -> Option<Arc<dyn SecureSessionStore>> {
    let url = redis_url()?;
    match RedisStores::connect(&url).await {
        Ok(stores) => Some(Arc::new(stores.secure_sessions())),
        Err(error) => {
            eprintln!("skipping redis secure-session integration: {error}");
            None
        }
    }
}

const PAGE_ENVELOPE: &[u8] = br#"{"component":"dashboard"}"#;

/// A plaintext page-protocol handler, as `phoenix-view` emits.
async fn page_handler(_request: Request) -> Response {
    let mut response = Response::new(StatusCode::OK, PAGE_ENVELOPE.to_vec());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PAGE_PROTOCOL_MEDIA_TYPE),
    );
    response
        .headers_mut()
        .insert(SECURE_ENCRYPTED_HEADER, HeaderValue::from_static("0"));
    response
}

/// One "server instance": a transport plus its routes.
fn instance(store: Arc<dyn SecureSessionStore>) -> (SecureTransport, Router) {
    let transport = SecureTransport::with_store(
        SecureTransportConfig {
            session_ttl: Duration::from_mins(5),
            ..SecureTransportConfig::default()
        },
        store,
    );
    let router = Routes::new()
        .post(
            transport.handshake_path().to_owned(),
            transport.handshake_handler(),
        )
        .get("/dashboard", page_handler)
        .with_middleware(transport.layer())
        .build()
        .expect("router builds");
    (transport, router)
}

/// Run the ECDH handshake against `router` and return the negotiated `key_id`.
///
/// The server never inspects whose point it is, only that it is on the curve,
/// and these tests never decrypt — they assert *which instance* can encrypt.
/// So a fixed valid point stands in for a browser's ephemeral key.
async fn handshake(router: &Router, path: &str) -> String {
    let body = serde_json::json!({
        "v": 1,
        "kex": "ECDH-P256",
        "hkdf": "HKDF-SHA256",
        "aead": "A256GCM",
        "client_public_key": base64_url(&CLIENT_POINT),
    })
    .to_string();

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let request = Request::from_parts(
        Method::POST,
        path.parse::<Uri>().expect("uri"),
        headers,
        body.into_bytes().into(),
    );
    let response = router.handle(request).await;
    assert_eq!(response.status(), StatusCode::OK, "handshake succeeded");
    let payload: serde_json::Value =
        serde_json::from_slice(response.body()).expect("handshake body");
    payload["key_id"].as_str().expect("key_id").to_owned()
}

/// The P-256 base point in uncompressed form — a valid public key to hand the
/// server's key agreement.
const CLIENT_POINT: [u8; 65] = [
    0x04, 0x6B, 0x17, 0xD1, 0xF2, 0xE1, 0x2C, 0x42, 0x47, 0xF8, 0xBC, 0xE6, 0xE5, 0x63, 0xA4, 0x40,
    0xF2, 0x77, 0x03, 0x7D, 0x81, 0x2D, 0xEB, 0x33, 0xA0, 0xF4, 0xA1, 0x39, 0x45, 0xD8, 0x98, 0xC2,
    0x96, 0x4F, 0xE3, 0x42, 0xE2, 0xFE, 0x1A, 0x7F, 0x9B, 0x8E, 0xE7, 0xEB, 0x4A, 0x7C, 0x0F, 0x9E,
    0x16, 0x2B, 0xCE, 0x33, 0x57, 0x6B, 0x31, 0x5E, 0xCE, 0xCB, 0xB6, 0x40, 0x68, 0x37, 0xBF, 0x51,
    0xF5,
];

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A page request carrying the secure directive for `key_id`.
fn secure_page_request(key_id: &str) -> Request {
    let mut headers = HeaderMap::new();
    headers.insert(SECURE_REQUEST_HEADER, HeaderValue::from_static("1"));
    headers.insert(
        SECURE_KEY_HEADER,
        HeaderValue::from_str(key_id).expect("key id header"),
    );
    Request::from_parts(
        Method::GET,
        "/dashboard".parse::<Uri>().expect("uri"),
        headers,
        Vec::new().into(),
    )
}

#[tokio::test]
async fn a_session_negotiated_on_one_instance_works_on_another() {
    let Some(store) = redis_store().await else {
        return;
    };
    let (transport_a, router_a) = instance(Arc::clone(&store));
    let (_transport_b, router_b) = instance(Arc::clone(&store));

    let key_id = handshake(&router_a, transport_a.handshake_path()).await;

    // The second instance never saw the handshake, yet it can encrypt for the
    // session — that is the whole point of the shared store.
    let response = router_b.handle(secure_page_request(&key_id)).await;
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        SECURE_CONTENT_TYPE,
        "the peer instance encrypted the reply"
    );
    assert_eq!(response.headers()[SECURE_ENCRYPTED_HEADER], "1");
    assert_ne!(
        response.body().as_ref(),
        PAGE_ENVELOPE,
        "the body is a frame, not the plaintext envelope"
    );
}

#[tokio::test]
async fn an_unknown_key_id_still_falls_back_to_plaintext() {
    let Some(store) = redis_store().await else {
        return;
    };
    let (_transport, router) = instance(store);

    // No handshake produced this id, so there is no session to encrypt for.
    // Reads fall back to plaintext rather than failing: the response path is
    // an enhancement, unlike an encrypted request body which fails closed.
    let response = router.handle(secure_page_request("never-negotiated")).await;
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        PAGE_PROTOCOL_MEDIA_TYPE
    );
    assert_eq!(response.body().as_ref(), PAGE_ENVELOPE);
}

#[tokio::test]
async fn the_in_process_store_deliberately_does_not_share() {
    // The control case: without a shared store, the same script fails, which
    // is why sticky routing is required in that deployment.
    let (transport_a, router_a) = instance(Arc::new(MemorySecureSessionStore::new(1000)));
    let (_transport_b, router_b) = instance(Arc::new(MemorySecureSessionStore::new(1000)));

    let key_id = handshake(&router_a, transport_a.handshake_path()).await;
    let response = router_b.handle(secure_page_request(&key_id)).await;
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        PAGE_PROTOCOL_MEDIA_TYPE,
        "a separate process has never heard of this session"
    );

    // The originating instance still has it.
    let own = router_a.handle(secure_page_request(&key_id)).await;
    assert_eq!(own.headers()[header::CONTENT_TYPE], SECURE_CONTENT_TYPE);
}
