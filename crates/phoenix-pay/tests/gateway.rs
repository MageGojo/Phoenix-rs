//! Integration tests against local fake gateways (plain-HTTP hyper servers
//! on 127.0.0.1) exercising the real providers end to end:
//!
//! - The fake `WeChat` gateway verifies the client's `Authorization`
//!   signature with the merchant public key (rejecting with 401 when it does
//!   not verify), serves `/v3/certificates` with an AES-256-GCM-encrypted
//!   platform certificate, and signs every response like the real platform.
//! - The fake Alipay gateway verifies the request `sign` with the app public
//!   key and returns RSA2-signed `alipay_trade_*_response` payloads.
//! - "Evil" variants sign responses with the wrong key: the providers MUST
//!   reject them.
//!
//! The test-side crypto is implemented directly on `ring` / `aes-gcm`, NOT
//! via the crate's internals, so it cross-checks the implementation.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::http::request::Parts;
use hyper::http::{HeaderMap, Response};
use hyper_util::rt::TokioIo;
use phoenix_pay::prelude::*;
use ring::signature::{
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_SHA256, RsaKeyPair, UnparsedPublicKey,
};
use tokio::net::TcpListener;

const MERCHANT_KEY: &str = include_str!("fixtures/wechat_merchant_key.pem");
const MERCHANT_PUB: &str = include_str!("fixtures/wechat_merchant_pub.pem");
const PLATFORM_KEY: &str = include_str!("fixtures/wechat_platform_key.pem");
const PLATFORM_CERT: &str = include_str!("fixtures/wechat_platform_cert.pem");
const ALIPAY_APP_KEY: &str = include_str!("fixtures/alipay_app_key.pem");
const ALIPAY_APP_PUB: &str = include_str!("fixtures/alipay_app_pub.pem");
const ALIPAY_PLATFORM_KEY: &str = include_str!("fixtures/alipay_platform_key.pem");
const ALIPAY_PLATFORM_PUB: &str = include_str!("fixtures/alipay_platform_pub.pem");

const PLATFORM_SERIAL: &str = "5157F09EFDC096DE15EBE81A47057A7232156733";
const API_V3_KEY: &[u8; 32] = b"0123456789abcdef0123456789abcdef";

// ---------------------------------------------------------------------------
// Test-side crypto helpers (independent of the crate's internals)
// ---------------------------------------------------------------------------

fn load_key_pair(pem: &str) -> RsaKeyPair {
    let mut reader = pem.as_bytes();
    for item in rustls_pemfile::read_all(&mut reader).flatten() {
        match item {
            rustls_pemfile::Item::Pkcs8Key(der) => {
                return RsaKeyPair::from_pkcs8(der.secret_pkcs8_der()).expect("pkcs8 key");
            }
            rustls_pemfile::Item::Pkcs1Key(der) => {
                return RsaKeyPair::from_der(der.secret_pkcs1_der()).expect("pkcs1 key");
            }
            _ => {}
        }
    }
    panic!("no private key in fixture");
}

fn rsa_sign_base64(key_pair: &RsaKeyPair, message: &[u8]) -> String {
    let rng = ring::rand::SystemRandom::new();
    let mut signature = vec![0_u8; key_pair.public().modulus_len()];
    key_pair
        .sign(&RSA_PKCS1_SHA256, &rng, message, &mut signature)
        .expect("sign");
    BASE64.encode(signature)
}

/// PKCS#1 `RSAPublicKey` DER out of a `PUBLIC KEY` (SPKI) PEM. The SPKI
/// prefix of an RSA key is `SEQUENCE { AlgorithmIdentifier, BIT STRING }`;
/// we walk to the BIT STRING payload with a minimal DER reader.
fn spki_pem_to_pkcs1(pem: &str) -> Vec<u8> {
    fn read_header(der: &[u8], at: usize) -> (u8, usize, usize) {
        let tag = der[at];
        let first = der[at + 1];
        if first < 0x80 {
            (tag, usize::from(first), at + 2)
        } else {
            let count = usize::from(first & 0x7f);
            let mut length = 0_usize;
            for offset in 0..count {
                length = (length << 8) | usize::from(der[at + 2 + offset]);
            }
            (tag, length, at + 2 + count)
        }
    }

    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = BASE64.decode(body).expect("spki base64");

    let (tag, _, inner) = read_header(&der, 0);
    assert_eq!(tag, 0x30, "outer SEQUENCE");
    let (tag, alg_len, alg_start) = read_header(&der, inner);
    assert_eq!(tag, 0x30, "AlgorithmIdentifier SEQUENCE");
    let (tag, bits_len, bits_start) = read_header(&der, alg_start + alg_len);
    assert_eq!(tag, 0x03, "BIT STRING");
    assert_eq!(der[bits_start], 0, "no unused bits");
    der[bits_start + 1..bits_start + bits_len].to_vec()
}

fn rsa_verify(spki_pem: &str, message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, spki_pem_to_pkcs1(spki_pem))
        .verify(message, signature)
        .is_ok()
}

fn aes_encrypt(nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    Aes256Gcm::new_from_slice(API_V3_KEY)
        .expect("cipher")
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("encrypt")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// ---------------------------------------------------------------------------
// Fake gateway plumbing
// ---------------------------------------------------------------------------

type Handler = dyn Fn(&Parts, &Bytes) -> Response<Full<Bytes>> + Send + Sync;

async fn spawn_gateway(handler: Arc<Handler>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let handler = Arc::clone(&handler);
                        async move {
                            let (parts, body) = request.into_parts();
                            let bytes = body.collect().await.expect("request body").to_bytes();
                            Ok::<_, Infallible>(handler(&parts, &bytes))
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    address
}

// ---------------------------------------------------------------------------
// Fake WeChat gateway
// ---------------------------------------------------------------------------

struct WechatGateway {
    /// Key used to sign responses; the honest gateway uses the platform key,
    /// the evil one signs with a key that does not match the certificate.
    response_key: RsaKeyPair,
    verified_requests: AtomicUsize,
    /// Own origin, filled in once the listener has a port; the bill ticket
    /// hands back an absolute download URL like the real gateway does.
    base_url: OnceLock<String>,
}

impl WechatGateway {
    fn signed_response(&self, status: u16, body: Bytes) -> Response<Full<Bytes>> {
        let timestamp = now().to_string();
        let nonce = "RESPNONCE";
        let mut message = Vec::new();
        message.extend_from_slice(timestamp.as_bytes());
        message.push(b'\n');
        message.extend_from_slice(nonce.as_bytes());
        message.push(b'\n');
        message.extend_from_slice(&body);
        message.push(b'\n');
        let signature = rsa_sign_base64(&self.response_key, &message);
        Response::builder()
            .status(status)
            .header("Wechatpay-Timestamp", timestamp)
            .header("Wechatpay-Nonce", nonce)
            .header("Wechatpay-Signature", signature)
            .header("Wechatpay-Serial", PLATFORM_SERIAL)
            .header("content-type", "application/json")
            .body(Full::new(body))
            .expect("response")
    }

    /// Verify the client's `Authorization` header against the merchant
    /// public key; this is what proves the provider's request signing works.
    fn verify_authorization(&self, parts: &Parts, body: &[u8]) -> bool {
        let Some(auth) = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let Some(rest) = auth.strip_prefix("WECHATPAY2-SHA256-RSA2048 ") else {
            return false;
        };
        let mut fields = BTreeMap::new();
        for pair in rest.split(',') {
            if let Some((key, value)) = pair.split_once('=') {
                fields.insert(key.trim(), value.trim().trim_matches('"'));
            }
        }
        let (Some(mchid), Some(nonce), Some(timestamp), Some(serial), Some(signature)) = (
            fields.get("mchid"),
            fields.get("nonce_str"),
            fields.get("timestamp"),
            fields.get("serial_no"),
            fields.get("signature"),
        ) else {
            return false;
        };
        if *mchid != "m1" || *serial != "MCHSERIAL1" {
            return false;
        }
        let path_and_query = parts
            .uri
            .path_and_query()
            .map_or_else(|| parts.uri.path().to_owned(), ToString::to_string);
        let message = format!(
            "{}\n{path_and_query}\n{timestamp}\n{nonce}\n{}\n",
            parts.method,
            std::str::from_utf8(body).unwrap_or("")
        );
        let Ok(signature) = BASE64.decode(signature) else {
            return false;
        };
        let valid = rsa_verify(MERCHANT_PUB, message.as_bytes(), &signature);
        if valid {
            self.verified_requests.fetch_add(1, Ordering::SeqCst);
        }
        valid
    }

    fn certificates_body() -> String {
        let nonce = *b"certnonce123";
        let ciphertext = aes_encrypt(&nonce, b"certificate", PLATFORM_CERT.as_bytes());
        serde_json::json!({
            "data": [{
                "serial_no": PLATFORM_SERIAL,
                "effective_time": "2026-01-01T00:00:00+08:00",
                "expire_time": "2031-01-01T00:00:00+08:00",
                "encrypt_certificate": {
                    "algorithm": "AEAD_AES_256_GCM",
                    "nonce": "certnonce123",
                    "associated_data": "certificate",
                    "ciphertext": BASE64.encode(ciphertext),
                }
            }]
        })
        .to_string()
    }

    fn handle(&self, parts: &Parts, body: &Bytes) -> Response<Full<Bytes>> {
        let path = parts.uri.path().to_owned();
        if !self.verify_authorization(parts, body) {
            let error = r#"{"code":"SIGN_ERROR","message":"bad client signature"}"#;
            return self.signed_response(401, Bytes::from_static(error.as_bytes()));
        }
        if path == "/v3/certificates" {
            return self.signed_response(200, Bytes::from(Self::certificates_body()));
        }
        if path == "/v3/pay/transactions/native" {
            let order: serde_json::Value = serde_json::from_slice(body).expect("order body");
            assert_eq!(order["appid"], "wx1");
            assert_eq!(order["mchid"], "m1");
            assert_eq!(order["amount"]["total"], 1234);
            assert_eq!(order["amount"]["currency"], "CNY");
            let body = serde_json::json!({
                "code_url": format!("weixin://wxpay/bizpayurl?pr={}", order["out_trade_no"].as_str().unwrap_or(""))
            });
            return self.signed_response(200, Bytes::from(body.to_string()));
        }
        if path == "/v3/pay/transactions/out-trade-no/GHOST" {
            let error = r#"{"code":"ORDER_NOT_EXIST","message":"order does not exist"}"#;
            return self.signed_response(404, Bytes::from_static(error.as_bytes()));
        }
        if let Some(out_trade_no) = path
            .strip_prefix("/v3/pay/transactions/out-trade-no/")
            .filter(|rest| !rest.contains('/'))
        {
            let body = serde_json::json!({
                "out_trade_no": out_trade_no,
                "transaction_id": format!("4200-{out_trade_no}"),
                "trade_state": "SUCCESS",
            });
            return self.signed_response(200, Bytes::from(body.to_string()));
        }
        if path.ends_with("/close") {
            return self.signed_response(204, Bytes::new());
        }
        if path == "/v3/refund/domestic/refunds" {
            let refund: serde_json::Value = serde_json::from_slice(body).expect("refund body");
            let out_refund_no = refund["out_refund_no"].as_str().unwrap_or("").to_owned();
            // A bank refund the gateway has accepted but not settled.
            let status = if out_refund_no.ends_with("-SLOW") {
                "PROCESSING"
            } else {
                "SUCCESS"
            };
            let body = serde_json::json!({
                "refund_id": format!("50000-{out_refund_no}"),
                "out_refund_no": out_refund_no,
                "out_trade_no": refund["out_trade_no"],
                "status": status,
                "amount": { "refund": refund["amount"]["refund"], "total": refund["amount"]["total"] },
            });
            return self.signed_response(200, Bytes::from(body.to_string()));
        }
        if let Some(out_refund_no) = path.strip_prefix("/v3/refund/domestic/refunds/") {
            if out_refund_no == "GHOST" {
                let error = r#"{"code":"RESOURCE_NOT_EXISTS","message":"no such refund"}"#;
                return self.signed_response(404, Bytes::from_static(error.as_bytes()));
            }
            let body = serde_json::json!({
                "refund_id": format!("50000-{out_refund_no}"),
                "out_refund_no": out_refund_no,
                "out_trade_no": "T-REFUND",
                "status": "SUCCESS",
                "amount": { "refund": 1234, "total": 1234 },
            });
            return self.signed_response(200, Bytes::from(body.to_string()));
        }
        if path == "/v3/bill/tradebill" {
            let csv = BILL_CSV;
            let digest =
                ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, csv.as_bytes());
            let hash = digest.as_ref().iter().fold(String::new(), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            });
            let query = parts.uri.query().unwrap_or_default();
            assert!(query.contains("bill_type=SUCCESS"), "query was {query}");
            let body = serde_json::json!({
                "hash_type": "SHA1",
                // A deliberately wrong digest when the caller asks for the
                // tampered day, so the client-side integrity check is exercised.
                "hash_value": if query.contains("bill_date=2026-07-26") { "0".repeat(40) } else { hash },
                "download_url": format!(
                    "{}/v3/billdownload/file?token=t",
                    self.base_url.get().map_or("", String::as_str)
                ),
            });
            return self.signed_response(200, Bytes::from(body.to_string()));
        }
        if path == "/v3/billdownload/file" {
            return Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .body(Full::new(Bytes::from_static(BILL_CSV.as_bytes())))
                .expect("response");
        }
        self.signed_response(
            404,
            Bytes::from_static(br#"{"code":"NOT_FOUND","message":"?"}"#),
        )
    }
}

/// A two-line trade bill in the real `WeChat` shape (Chinese headers, backtick
/// cell prefixes).
const BILL_CSV: &str = "交易时间,微信订单号,商户订单号,交易状态,订单金额,退款金额\n\
`2026-07-25 10:00:00,`4200001,`T-BILL-1,SUCCESS,12.34,0.00\n\
`2026-07-25 11:00:00,`4200002,`T-BILL-2,SUCCESS,0.05,0.00\n";

fn wechat_config(with_cert_file: bool) -> WechatNativeConfig {
    WechatNativeConfig {
        app_id: "wx1".to_owned(),
        mch_id: "m1".to_owned(),
        mch_serial_no: "MCHSERIAL1".to_owned(),
        api_v3_key: Secret::new(std::str::from_utf8(API_V3_KEY).expect("key utf8")),
        private_key_path: fixture("wechat_merchant_key.pem"),
        platform_cert_path: with_cert_file.then(|| fixture("wechat_platform_cert.pem")),
        notify_url: "https://shop.example.com/pay/notify/wechat".to_owned(),
        refund_notify_url: Some("https://shop.example.com/pay/notify/wechat/refund".to_owned()),
    }
}

async fn spawn_wechat(response_key_pem: &str) -> (SocketAddr, Arc<WechatGateway>) {
    let gateway = Arc::new(WechatGateway {
        response_key: load_key_pair(response_key_pem),
        verified_requests: AtomicUsize::new(0),
        base_url: OnceLock::new(),
    });
    let handler = Arc::clone(&gateway);
    let address = spawn_gateway(Arc::new(move |parts: &Parts, body: &Bytes| {
        handler.handle(parts, body)
    }))
    .await;
    let _ = gateway.base_url.set(format!("http://{address}"));
    (address, gateway)
}

#[tokio::test]
async fn wechat_create_query_close_against_fake_gateway() {
    let (address, gateway) = spawn_wechat(PLATFORM_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );

    let order = CreateOrder::new("T100", Amount::cny(1234), "会员月卡");
    let intent = provider.create(&order).await.expect("create");
    assert_eq!(intent.provider, "wechat_native");
    assert_eq!(
        intent.action,
        PaymentAction::QrCode("weixin://wxpay/bizpayurl?pr=T100".to_owned())
    );

    assert_eq!(provider.query("T100").await, Ok(PaymentStatus::Paid));
    provider.close_order("T100").await.expect("close");
    assert!(matches!(
        provider.query("GHOST").await,
        Err(PayError::OrderNotFound { .. })
    ));

    // The fake gateway actually verified our Authorization signatures
    // (certificates download + create + 2x query + close).
    assert!(gateway.verified_requests.load(Ordering::SeqCst) >= 5);
}

#[tokio::test]
async fn wechat_refund_and_refund_query_against_fake_gateway() {
    let (address, gateway) = spawn_wechat(PLATFORM_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );

    let receipt = provider
        .refund(&RefundOrder::full("T-REFUND", "R-1", Amount::cny(1234)).reason("买错了"))
        .await
        .expect("refund");
    assert_eq!(receipt.provider, "wechat_native");
    assert_eq!(receipt.out_trade_no, "T-REFUND");
    assert_eq!(receipt.out_refund_no, "R-1");
    assert_eq!(receipt.refund_id.as_deref(), Some("50000-R-1"));
    assert_eq!(receipt.amount, Amount::cny(1234));
    assert_eq!(receipt.status, RefundStatus::Succeeded);

    // A partial refund carries the smaller amount but the full order total.
    let partial = provider
        .refund(&RefundOrder::partial(
            "T-REFUND",
            "R-2",
            Amount::cny(34),
            Amount::cny(1234),
        ))
        .await
        .expect("partial refund");
    assert_eq!(partial.amount, Amount::cny(34));

    // A refund the gateway accepted but has not settled stays Processing.
    let slow = provider
        .refund(&RefundOrder::full(
            "T-REFUND",
            "R-3-SLOW",
            Amount::cny(1234),
        ))
        .await
        .expect("slow refund");
    assert_eq!(slow.status, RefundStatus::Processing);

    let queried = provider
        .query_refund("T-REFUND", "R-1")
        .await
        .expect("query refund");
    assert_eq!(queried.status, RefundStatus::Succeeded);
    assert_eq!(queried.out_refund_no, "R-1");

    assert!(matches!(
        provider.query_refund("T-REFUND", "GHOST").await,
        Err(PayError::RefundNotFound { .. })
    ));

    // Every one of those calls was signature-verified by the fake gateway.
    assert!(gateway.verified_requests.load(Ordering::SeqCst) >= 5);
}

#[tokio::test]
async fn wechat_bill_download_verifies_the_published_digest() {
    let (address, _gateway) = spawn_wechat(PLATFORM_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );

    let bill = provider.download_bill("2026-07-25").await.expect("bill");
    assert_eq!(bill.provider, "wechat_native");
    assert_eq!(bill.date, "2026-07-25");
    assert_eq!(bill.entries.len(), 2);
    assert_eq!(bill.entries[0].out_trade_no, "T-BILL-1");
    assert_eq!(bill.entries[0].amount, Amount::cny(1234));
    assert_eq!(bill.entries[0].transaction_id.as_deref(), Some("4200001"));
    assert_eq!(bill.net_total().expect("net"), Amount::cny(1239));

    // The gateway publishes a digest that does not match the file it serves:
    // the download must be rejected rather than parsed.
    let error = provider
        .download_bill("2026-07-26")
        .await
        .expect_err("digest mismatch must fail");
    assert!(
        matches!(&error, PayError::Reconcile(message) if message.contains("digest")),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn wechat_rejects_gateway_with_bad_response_signature() {
    // The evil gateway signs responses with the merchant key, which does not
    // match the platform certificate it serves.
    let (address, _gateway) = spawn_wechat(MERCHANT_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );
    let order = CreateOrder::new("T200", Amount::cny(1234), "tea");
    let error = provider.create(&order).await.expect_err("must reject");
    assert!(
        matches!(&error, PayError::Gateway(message) if message.contains("signature")),
        "unexpected error: {error:?}"
    );
}

fn wechat_notify_request(tamper: bool, serial: &str) -> NotifyRequest {
    let resource_plain = serde_json::json!({
        "out_trade_no": "T300",
        "transaction_id": "4200-T300",
        "trade_state": "SUCCESS",
        "amount": { "total": 1234, "currency": "CNY" },
    })
    .to_string();
    let nonce = *b"notifynonce1";
    let ciphertext = aes_encrypt(&nonce, b"transaction", resource_plain.as_bytes());
    let mut body = serde_json::json!({
        "id": "EV-2026",
        "event_type": "TRANSACTION.SUCCESS",
        "resource_type": "encrypt-resource",
        "resource": {
            "algorithm": "AEAD_AES_256_GCM",
            "ciphertext": BASE64.encode(ciphertext),
            "nonce": "notifynonce1",
            "associated_data": "transaction",
        }
    })
    .to_string();

    let timestamp = now().to_string();
    let platform = load_key_pair(PLATFORM_KEY);
    let mut message = Vec::new();
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(b"\nNONCE1\n");
    message.extend_from_slice(body.as_bytes());
    message.push(b'\n');
    let signature = rsa_sign_base64(&platform, &message);
    if tamper {
        body = body.replace("TRANSACTION.SUCCESS", "TRANSACTION.HACKED");
    }

    let mut headers = HeaderMap::new();
    headers.insert("Wechatpay-Timestamp", timestamp.parse().expect("header"));
    headers.insert("Wechatpay-Nonce", "NONCE1".parse().expect("header"));
    headers.insert("Wechatpay-Signature", signature.parse().expect("header"));
    headers.insert("Wechatpay-Serial", serial.parse().expect("header"));
    NotifyRequest::new(headers, Bytes::from(body))
}

/// A signed, encrypted `WeChat` refund callback. `event_type` is a parameter so
/// a payment callback can be aimed at the refund route.
fn wechat_refund_notify_request(event_type: &str, refund_status: &str) -> NotifyRequest {
    let resource_plain = serde_json::json!({
        "mchid": "m1",
        "out_trade_no": "T-REFUND",
        "transaction_id": "4200-T-REFUND",
        "out_refund_no": "R-1",
        "refund_id": "50000-R-1",
        "refund_status": refund_status,
        "amount": { "refund": 1234, "total": 1234 },
    })
    .to_string();
    let nonce = *b"refundnonce1";
    let ciphertext = aes_encrypt(&nonce, b"refund", resource_plain.as_bytes());
    let body = serde_json::json!({
        "id": "EV-REFUND",
        "event_type": event_type,
        "resource_type": "encrypt-resource",
        "resource": {
            "algorithm": "AEAD_AES_256_GCM",
            "ciphertext": BASE64.encode(ciphertext),
            "nonce": "refundnonce1",
            "associated_data": "refund",
        }
    })
    .to_string();

    let timestamp = now().to_string();
    let platform = load_key_pair(PLATFORM_KEY);
    let mut message = Vec::new();
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(b"\nNONCE1\n");
    message.extend_from_slice(body.as_bytes());
    message.push(b'\n');
    let signature = rsa_sign_base64(&platform, &message);

    let mut headers = HeaderMap::new();
    headers.insert("Wechatpay-Timestamp", timestamp.parse().expect("header"));
    headers.insert("Wechatpay-Nonce", "NONCE1".parse().expect("header"));
    headers.insert("Wechatpay-Signature", signature.parse().expect("header"));
    headers.insert("Wechatpay-Serial", PLATFORM_SERIAL.parse().expect("header"));
    NotifyRequest::new(headers, Bytes::from(body))
}

#[tokio::test]
async fn wechat_refund_notify_verifies_decrypts_and_refuses_the_wrong_event() {
    let provider = WechatNativeProvider::new(wechat_config(true));

    let event = provider
        .verify_refund_notify(&wechat_refund_notify_request("REFUND.SUCCESS", "SUCCESS"))
        .await
        .expect("refund notify");
    assert_eq!(event.out_trade_no, "T-REFUND");
    assert_eq!(event.out_refund_no, "R-1");
    assert_eq!(event.refund_id.as_deref(), Some("50000-R-1"));
    assert_eq!(event.amount, Amount::cny(1234));
    assert_eq!(event.status, RefundStatus::Succeeded);

    // `ABNORMAL` means the money did not go back and a human must act, so it
    // is a failed refund rather than one left pending forever.
    let abnormal = provider
        .verify_refund_notify(&wechat_refund_notify_request("REFUND.ABNORMAL", "ABNORMAL"))
        .await
        .expect("refund notify");
    assert_eq!(abnormal.status, RefundStatus::Failed);

    // A payment callback delivered to the refund route must be refused: the
    // two resources have different shapes and applying one as the other would
    // corrupt a refund record.
    let wrong = provider
        .verify_refund_notify(&wechat_refund_notify_request(
            "TRANSACTION.SUCCESS",
            "SUCCESS",
        ))
        .await
        .expect_err("a payment event is not a refund event");
    assert!(
        matches!(&wrong, PayError::InvalidNotify(message) if message.contains("REFUND")),
        "unexpected error: {wrong:?}"
    );
}

#[tokio::test]
async fn wechat_refund_notify_rejects_an_unsigned_payload() {
    let provider = WechatNativeProvider::new(wechat_config(true));
    let unsigned =
        NotifyRequest::from_body(serde_json::json!({ "event_type": "REFUND.SUCCESS" }).to_string());
    assert!(matches!(
        provider.verify_refund_notify(&unsigned).await,
        Err(PayError::InvalidNotify(_))
    ));
}

#[tokio::test]
async fn wechat_notify_verifies_decrypts_and_rejects_tampering() {
    // platform_cert_path set: verification is fully offline.
    let provider = WechatNativeProvider::new(wechat_config(true));

    let event = provider
        .verify_notify(&wechat_notify_request(false, PLATFORM_SERIAL))
        .await
        .expect("verified notify");
    assert_eq!(event.out_trade_no, "T300");
    assert_eq!(event.transaction_id.as_deref(), Some("4200-T300"));
    assert_eq!(event.status, PaymentStatus::Paid);
    assert!(event.raw.contains("\"trade_state\":\"SUCCESS\""));

    let error = provider
        .verify_notify(&wechat_notify_request(true, PLATFORM_SERIAL))
        .await
        .expect_err("tampered body must fail");
    assert!(matches!(error, PayError::InvalidNotify(_)));

    let error = provider
        .verify_notify(&wechat_notify_request(false, "DEADBEEF"))
        .await
        .expect_err("unknown serial must fail");
    assert!(matches!(error, PayError::InvalidNotify(_)));
}

#[tokio::test]
async fn wechat_notify_with_downloaded_certificates() {
    let (address, _gateway) = spawn_wechat(PLATFORM_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );
    let event = provider
        .verify_notify(&wechat_notify_request(false, PLATFORM_SERIAL))
        .await
        .expect("verified notify with downloaded certs");
    assert_eq!(event.status, PaymentStatus::Paid);
}

// ---------------------------------------------------------------------------
// Fake Alipay gateway
// ---------------------------------------------------------------------------

struct AlipayGateway {
    response_key: RsaKeyPair,
    verified_requests: AtomicUsize,
    /// Own origin, filled in once the listener has a port, so the bill
    /// download URL points back at this fake server.
    base_url: OnceLock<String>,
}

impl AlipayGateway {
    fn verify_request(&self, params: &BTreeMap<String, String>) -> bool {
        let Some(signature) = params.get("sign") else {
            return false;
        };
        let Ok(signature) = BASE64.decode(signature) else {
            return false;
        };
        let content = params
            .iter()
            .filter(|(key, value)| key.as_str() != "sign" && !value.is_empty())
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let valid = rsa_verify(ALIPAY_APP_PUB, content.as_bytes(), &signature);
        if valid {
            self.verified_requests.fetch_add(1, Ordering::SeqCst);
        }
        valid
    }

    fn respond(&self, method: &str, content: &serde_json::Value) -> Response<Full<Bytes>> {
        let content = content.to_string();
        let signature = rsa_sign_base64(&self.response_key, content.as_bytes());
        let key = format!("{}_response", method.replace('.', "_"));
        let body = format!(r#"{{"{key}":{content},"sign":"{signature}"}}"#);
        Response::builder()
            .status(200)
            .header("content-type", "application/json;charset=utf-8")
            .body(Full::new(Bytes::from(body)))
            .expect("response")
    }

    #[allow(clippy::too_many_lines, reason = "one arm per fake OpenAPI method")]
    fn handle(&self, body: &Bytes) -> Response<Full<Bytes>> {
        let params: BTreeMap<String, String> =
            serde_urlencoded::from_bytes(body).expect("form body");
        let method = params.get("method").cloned().unwrap_or_default();
        assert_eq!(params.get("charset").map(String::as_str), Some("utf-8"));
        assert_eq!(params.get("sign_type").map(String::as_str), Some("RSA2"));
        if !self.verify_request(&params) {
            return self.respond(
                &method,
                &serde_json::json!({
                    "code": "40002", "msg": "Invalid Arguments",
                    "sub_code": "isv.invalid-signature", "sub_msg": "bad sign",
                }),
            );
        }
        let biz: serde_json::Value = params
            .get("biz_content")
            .map(|content| serde_json::from_str(content).expect("biz_content"))
            .unwrap_or_default();
        let out_trade_no = biz["out_trade_no"].as_str().unwrap_or("").to_owned();
        match method.as_str() {
            "alipay.trade.precreate" => {
                assert_eq!(biz["total_amount"], "12.34");
                assert_eq!(params.get("app_id").map(String::as_str), Some("2021001"));
                assert!(params.contains_key("notify_url"));
                self.respond(
                    &method,
                    &serde_json::json!({
                        "code": "10000", "msg": "Success",
                        "out_trade_no": out_trade_no,
                        "qr_code": format!("https://qr.alipay.com/{out_trade_no}"),
                    }),
                )
            }
            // Both `query` and `refund` answer the same "no such trade" body.
            "alipay.trade.query" | "alipay.trade.refund" if out_trade_no == "GHOST" => self
                .respond(
                    &method,
                    &serde_json::json!({
                        "code": "40004", "msg": "Business Failed",
                        "sub_code": "ACQ.TRADE_NOT_EXIST", "sub_msg": "trade not exist",
                    }),
                ),
            "alipay.trade.query" => self.respond(
                &method,
                &serde_json::json!({
                    "code": "10000", "msg": "Success",
                    "out_trade_no": out_trade_no,
                    "trade_no": format!("2026{out_trade_no}"),
                    "trade_status": "TRADE_SUCCESS",
                    "total_amount": "12.34",
                }),
            ),
            "alipay.trade.close" => self.respond(
                &method,
                &serde_json::json!({
                    "code": "10000", "msg": "Success",
                    "out_trade_no": out_trade_no,
                }),
            ),
            "alipay.trade.refund" => {
                // Alipay requires the per-refund idempotency key.
                assert!(
                    biz["out_request_no"].is_string(),
                    "out_request_no must always be sent"
                );
                self.respond(
                    &method,
                    &serde_json::json!({
                        "code": "10000", "msg": "Success",
                        "out_trade_no": out_trade_no,
                        "trade_no": format!("2026{out_trade_no}"),
                        "out_request_no": biz["out_request_no"],
                        "refund_fee": biz["refund_amount"],
                    }),
                )
            }
            "alipay.trade.fastpay.refund.query" => {
                let out_request_no = biz["out_request_no"].as_str().unwrap_or("");
                if out_request_no == "GHOST" {
                    // Alipay answers "success" with an empty body for a refund
                    // it does not know; the driver must not read that as a
                    // zero-amount refund.
                    return self.respond(
                        &method,
                        &serde_json::json!({ "code": "10000", "msg": "Success" }),
                    );
                }
                self.respond(
                    &method,
                    &serde_json::json!({
                        "code": "10000", "msg": "Success",
                        "out_trade_no": out_trade_no,
                        "trade_no": format!("2026{out_trade_no}"),
                        "out_request_no": out_request_no,
                        "refund_amount": "12.34",
                        "refund_status": "REFUND_SUCCESS",
                    }),
                )
            }
            "alipay.data.dataservice.bill.downloadurl.query" => self.respond(
                &method,
                &serde_json::json!({
                    "code": "10000", "msg": "Success",
                    "bill_download_url": format!(
                        "{}/billdownload?bizType=trade",
                        self.base_url.get().map_or("", String::as_str)
                    ),
                }),
            ),
            _ => self.respond(
                &method,
                &serde_json::json!({ "code": "40004", "msg": "unknown method" }),
            ),
        }
    }
}

/// A ZIP holding what Alipay actually serves: a **GBK** trade-detail CSV plus
/// a summary member the parser must not mistake for it.
///
/// Built with the stored method so the fixture needs no compressor, and with
/// the `#`-prefixed comment block the real export carries.
fn alipay_bill_archive() -> Vec<u8> {
    // 支付宝交易号,商户订单号,业务类型,订单金额（元）  — GBK
    let mut detail: Vec<u8> = Vec::new();
    detail.extend_from_slice("#支付宝交易明细查询\n#-----\n".as_bytes());
    detail.extend_from_slice(
        b"\xD6\xA7\xB8\xB6\xB1\xA6\xBD\xBB\xD2\xD7\xBA\xC5,          \xC9\xCC\xBB\xA7\xB6\xA9\xB5\xA5\xBA\xC5,          \xD2\xB5\xCE\xF1\xC0\xE0\xD0\xCD,          \xB6\xA9\xB5\xA5\xBD\xF0\xB6\xEE\xA3\xA8\xD4\xAA\xA3\xA9\n",
    );
    // 交易 (paid) and 退款 (refunded), both in GBK.
    detail.extend_from_slice(b"2026T1,T-BILL-1,\xBD\xBB\xD2\xD7,12.34\n");
    detail.extend_from_slice(b"2026T2,T-BILL-2,\xCD\xCB\xBF\xEE,0.05\n");
    detail.extend_from_slice("#-----\n#汇总\n".as_bytes());

    // The summary member has none of the columns we match on.
    let mut summary: Vec<u8> = Vec::new();
    summary.extend_from_slice("#支付宝业务汇总\n".as_bytes());
    // 笔数,金额 — GBK
    summary.extend_from_slice(b"\xB1\xCA\xCA\xFD,\xBD\xF0\xB6\xEE\n2,12.39\n");

    stored_zip(&[("detail.csv", &detail), ("summary.csv", &summary)])
}

/// Build a ZIP whose members are stored uncompressed.
fn stored_zip(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut directory: Vec<u8> = Vec::new();
    for (name, data) in members {
        let local_offset = u32::try_from(out.len()).expect("offset");
        let size = u32::try_from(data.len()).expect("size");
        let name_len = u16::try_from(name.len()).expect("name length");

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, method 0
        out.extend_from_slice(&[0; 8]); // time, date, crc (unchecked)
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        directory.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        directory.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]); // versions, flags, method 0
        directory.extend_from_slice(&[0; 8]); // time, date, crc (unchecked)
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&name_len.to_le_bytes());
        directory.extend_from_slice(&[0; 8]); // extra, comment, disk, attrs
        directory.extend_from_slice(&[0; 4]); // external attributes
        directory.extend_from_slice(&local_offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }

    let directory_offset = u32::try_from(out.len()).expect("offset");
    let directory_len = u32::try_from(directory.len()).expect("length");
    let count = u16::try_from(members.len()).expect("count");
    out.extend_from_slice(&directory);
    out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&directory_len.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

fn alipay_config(gateway_url: String) -> AlipayF2FConfig {
    AlipayF2FConfig {
        app_id: "2021001".to_owned(),
        gateway_url,
        app_private_key: Secret::new(ALIPAY_APP_KEY),
        alipay_public_key: Secret::new(ALIPAY_PLATFORM_PUB),
        sign_type: "RSA2".to_owned(),
        notify_url: "https://shop.example.com/pay/notify/alipay".to_owned(),
        app_cert_path: None,
        alipay_root_cert_path: None,
    }
}

async fn spawn_alipay(response_key_pem: &str) -> (SocketAddr, Arc<AlipayGateway>) {
    let gateway = Arc::new(AlipayGateway {
        response_key: load_key_pair(response_key_pem),
        verified_requests: AtomicUsize::new(0),
        base_url: OnceLock::new(),
    });
    let handler = Arc::clone(&gateway);
    let address = spawn_gateway(Arc::new(move |parts: &Parts, body: &Bytes| {
        if parts.uri.path() == "/billdownload" {
            return Response::builder()
                .status(200)
                .header("content-type", "application/zip")
                .body(Full::new(Bytes::from(alipay_bill_archive())))
                .expect("response");
        }
        handler.handle(body)
    }))
    .await;
    let _ = gateway.base_url.set(format!("http://{address}"));
    (address, gateway)
}

#[tokio::test]
async fn alipay_create_query_close_against_fake_gateway() {
    let (address, gateway) = spawn_alipay(ALIPAY_PLATFORM_KEY).await;
    // Plain `new`: the default transport reads gateway_url from the config.
    let provider = AlipayF2FProvider::new(alipay_config(format!("http://{address}/gateway.do")));

    let order = CreateOrder::new("A100", Amount::cny(1234), "会员月卡");
    let intent = provider.create(&order).await.expect("create");
    assert_eq!(intent.provider, "alipay_f2f");
    assert_eq!(
        intent.action,
        PaymentAction::QrCode("https://qr.alipay.com/A100".to_owned())
    );

    assert_eq!(provider.query("A100").await, Ok(PaymentStatus::Paid));
    provider.close_order("A100").await.expect("close");
    assert!(matches!(
        provider.query("GHOST").await,
        Err(PayError::OrderNotFound { .. })
    ));
    assert!(gateway.verified_requests.load(Ordering::SeqCst) >= 4);
}

#[tokio::test]
async fn alipay_refund_query_and_bill_url_against_fake_gateway() {
    let (address, _gateway) = spawn_alipay(ALIPAY_PLATFORM_KEY).await;
    let provider = AlipayF2FProvider::with_transport(
        alipay_config(format!("http://{address}/gateway.do")),
        Arc::new(HyperPayHttp::new()),
    );

    let receipt = provider
        .refund(&RefundOrder::full("T300", "R-1", Amount::cny(1234)).reason("取消订单"))
        .await
        .expect("refund");
    assert_eq!(receipt.provider, "alipay_f2f");
    assert_eq!(receipt.out_refund_no, "R-1");
    assert_eq!(receipt.amount, Amount::cny(1234));
    // `alipay.trade.refund` settles synchronously — there is no pending state.
    assert_eq!(receipt.status, RefundStatus::Succeeded);
    assert_eq!(receipt.refund_id.as_deref(), Some("2026T300"));

    let queried = provider
        .query_refund("T300", "R-1")
        .await
        .expect("query refund");
    assert_eq!(queried.status, RefundStatus::Succeeded);
    assert_eq!(queried.amount, Amount::cny(1234));

    assert!(matches!(
        provider
            .refund(&RefundOrder::full("GHOST", "R-9", Amount::cny(1)))
            .await,
        Err(PayError::OrderNotFound { .. })
    ));
    assert!(
        matches!(
            provider.query_refund("T300", "GHOST").await,
            Err(PayError::RefundNotFound { .. })
        ),
        "an empty successful answer means the refund is unknown, not zero"
    );

    // The raw signed URL stays available for archiving the original file.
    let url = provider.bill_url("2026-07-25").await.expect("bill url");
    assert!(url.contains("/billdownload"));
}

#[tokio::test]
async fn alipay_bill_download_unzips_and_parses_the_gbk_detail_member() {
    let (address, _gateway) = spawn_alipay(ALIPAY_PLATFORM_KEY).await;
    let provider = AlipayF2FProvider::with_transport(
        alipay_config(format!("http://{address}/gateway.do")),
        Arc::new(HyperPayHttp::new()),
    );

    let bill = provider.download_bill("2026-07-25").await.expect("bill");
    assert_eq!(bill.provider, "alipay_f2f");
    assert_eq!(bill.date, "2026-07-25");
    // The summary member must not win: it has none of the columns we match on.
    assert_eq!(bill.entries.len(), 2);
    assert_eq!(bill.entries[0].out_trade_no, "T-BILL-1");
    assert_eq!(bill.entries[0].transaction_id.as_deref(), Some("2026T1"));
    assert_eq!(bill.entries[0].amount, Amount::cny(1234));
    assert_eq!(bill.entries[0].status, PaymentStatus::Paid);
    // GBK 退款 in the 业务类型 column, matched without transcoding the file.
    assert_eq!(bill.entries[1].status, PaymentStatus::Refunded);
    assert_eq!(bill.net_total().expect("net"), Amount::cny(1234));
}

#[tokio::test]
async fn alipay_rejects_gateway_with_bad_response_signature() {
    // The evil gateway signs responses with the app key instead of the
    // Alipay platform key.
    let (address, _gateway) = spawn_alipay(ALIPAY_APP_KEY).await;
    let provider = AlipayF2FProvider::new(alipay_config(format!("http://{address}/gateway.do")));
    let order = CreateOrder::new("A200", Amount::cny(1234), "tea");
    let error = provider.create(&order).await.expect_err("must reject");
    assert!(
        matches!(&error, PayError::Gateway(message) if message.contains("signature")),
        "unexpected error: {error:?}"
    );
}

fn alipay_notify_body(pairs: &[(&str, &str)], signer_pem: &str) -> String {
    let mut params: BTreeMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let content = params
        .iter()
        .filter(|(key, _)| key.as_str() != "sign" && key.as_str() != "sign_type")
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let signer = load_key_pair(signer_pem);
    params.insert(
        "sign".to_owned(),
        rsa_sign_base64(&signer, content.as_bytes()),
    );
    serde_urlencoded::to_string(&params).expect("form")
}

#[tokio::test]
async fn alipay_notify_verifies_and_rejects_tampering() {
    let provider = AlipayF2FProvider::new(alipay_config(
        "https://openapi.alipay.com/gateway.do".to_owned(),
    ));
    let pairs = [
        ("app_id", "2021001"),
        ("out_trade_no", "A300"),
        ("trade_no", "2026A300"),
        ("trade_status", "TRADE_SUCCESS"),
        ("total_amount", "12.34"),
        ("sign_type", "RSA2"),
        ("subject", "会员月卡"),
    ];

    let body = alipay_notify_body(&pairs, ALIPAY_PLATFORM_KEY);
    let event = provider
        .verify_notify(&NotifyRequest::from_body(body.clone()))
        .await
        .expect("verified notify");
    assert_eq!(event.out_trade_no, "A300");
    assert_eq!(event.transaction_id.as_deref(), Some("2026A300"));
    assert_eq!(event.status, PaymentStatus::Paid);
    assert_eq!(event.raw, body);

    // Tampering with any signed field must fail verification.
    let tampered = body.replace("A300", "A999");
    assert!(matches!(
        provider
            .verify_notify(&NotifyRequest::from_body(tampered))
            .await,
        Err(PayError::InvalidNotify(_))
    ));

    // A notify signed with the wrong key must fail.
    let forged = alipay_notify_body(&pairs, ALIPAY_APP_KEY);
    assert!(matches!(
        provider
            .verify_notify(&NotifyRequest::from_body(forged))
            .await,
        Err(PayError::InvalidNotify(_))
    ));

    // A valid signature for a different app_id must be rejected.
    let mut other_app = pairs;
    other_app[0] = ("app_id", "9999999");
    let cross = alipay_notify_body(&other_app, ALIPAY_PLATFORM_KEY);
    assert!(matches!(
        provider
            .verify_notify(&NotifyRequest::from_body(cross))
            .await,
        Err(PayError::InvalidNotify(_))
    ));
}

// ---------------------------------------------------------------------------
// Full manager flow over the fake WeChat gateway
// ---------------------------------------------------------------------------

#[tokio::test]
async fn manager_flow_with_wechat_provider() {
    let (address, _gateway) = spawn_wechat(PLATFORM_KEY).await;
    let provider = WechatNativeProvider::with_transport(
        wechat_config(false),
        Arc::new(HyperPayHttp::new()),
        format!("http://{address}"),
    );
    let manager = PayManager::builder().provider(Arc::new(provider)).build();

    let intent = manager
        .create(
            "wechat_native",
            CreateOrder::new("T300", Amount::cny(1234), "tea"),
        )
        .await
        .expect("create");
    assert!(matches!(intent.action, PaymentAction::QrCode(_)));

    let outcome = manager
        .handle_notify(
            "wechat_native",
            wechat_notify_request(false, PLATFORM_SERIAL),
        )
        .await
        .expect("notify");
    assert!(matches!(outcome, NotifyOutcome::Processed(_)));
    let record = manager
        .find_order("wechat_native", "T300")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, PaymentStatus::Paid);
    assert!(
        record
            .notify_payload
            .as_deref()
            .is_some_and(|raw| raw.contains("\"trade_state\":\"SUCCESS\"")),
        "store must keep the decrypted, verified payload"
    );
}
