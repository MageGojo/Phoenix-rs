//! Integration tests for [`S3Disk`] against a local fake S3 server (a
//! plain-HTTP hyper server on 127.0.0.1).
//!
//! The fake server independently re-implements AWS Signature V4 verification
//! with raw `hmac` / `sha2` (NOT via the crate's internals), so it genuinely
//! cross-checks that the driver's signatures are real and would be accepted by
//! any conformant S3 endpoint:
//!
//! - Header-auth PUT/GET/DELETE/HEAD round-trips (`Authorization: AWS4-…`).
//! - The `x-amz-content-sha256` header must match the received body.
//! - Presigned GET/PUT URLs fetched by a plain HTTP client are accepted.
//! - A tampered header signature or presigned signature is rejected with 403.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::{BodyExt, Full};
use hyper::http::request::Parts;
use hyper::http::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

use phoenix_storage::{
    Addressing, BoxFuture, HyperS3Http, S3Config, S3Disk, S3Http, S3Request, S3Response, Storage,
    StorageError,
};

const ACCESS_KEY: &str = "TESTACCESSKEYID";
const SECRET_KEY: &str = "TESTSECRETACCESSKEY0123456789";
const REGION: &str = "us-east-1";
const BUCKET: &str = "uploads";

// ---------------------------------------------------------------------------
// Test-side SigV4 verification (independent of the crate internals)
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(data);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    to_hex(&Sha256::digest(data))
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// `signature = hex(HMAC(signing_key(scope), string_to_sign))`.
fn compute_signature(date: &str, region: &str, service: &str, string_to_sign: &str) -> String {
    to_hex(&hmac_sha256(
        &signing_key(SECRET_KEY, date, region, service),
        string_to_sign.as_bytes(),
    ))
}

fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    )
}

/// Parse a credential of the form `access/date/region/service/aws4_request`.
fn parse_scope(credential: &str) -> Result<(String, String, String), String> {
    let scope: Vec<&str> = credential.split('/').collect();
    match scope.as_slice() {
        [_access, date, region, service, "aws4_request"] => Ok((
            (*date).to_owned(),
            (*region).to_owned(),
            (*service).to_owned(),
        )),
        _ => Err("malformed credential scope".to_owned()),
    }
}

/// Verify an `Authorization: AWS4-HMAC-SHA256 …` header signature.
fn verify_header_auth(parts: &Parts, body: &[u8]) -> Result<(), String> {
    let header = |name: &str| -> Option<String> {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };

    let authorization = header("authorization").ok_or("missing Authorization")?;
    let rest = authorization
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or("bad algorithm")?;

    let (mut credential, mut signed_headers, mut provided) = (None, None, None);
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("Credential=") {
            credential = Some(value.to_owned());
        } else if let Some(value) = part.strip_prefix("SignedHeaders=") {
            signed_headers = Some(value.to_owned());
        } else if let Some(value) = part.strip_prefix("Signature=") {
            provided = Some(value.to_owned());
        }
    }
    let credential = credential.ok_or("missing Credential")?;
    let signed_headers = signed_headers.ok_or("missing SignedHeaders")?;
    let provided = provided.ok_or("missing Signature")?;
    let (date_stamp, region, service) = parse_scope(&credential)?;

    // The content hash header must describe the received body.
    let content_sha = header("x-amz-content-sha256").ok_or("missing content sha")?;
    if content_sha != sha256_hex(body) {
        return Err("x-amz-content-sha256 mismatch".to_owned());
    }
    let amz_date = header("x-amz-date").ok_or("missing x-amz-date")?;

    // Rebuild the canonical headers block from the signed header names.
    let mut names: Vec<String> = signed_headers.split(';').map(str::to_owned).collect();
    names.sort();
    let mut canonical_headers = String::new();
    for name in &names {
        let value = header(name).ok_or_else(|| format!("missing signed header {name}"))?;
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value.trim());
        canonical_headers.push('\n');
    }

    let canonical_request = format!(
        "{}\n{}\n\n{canonical_headers}\n{signed_headers}\n{content_sha}",
        parts.method.as_str(),
        parts.uri.path(),
    );
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let expected = compute_signature(
        &date_stamp,
        &region,
        &service,
        &string_to_sign(&amz_date, &scope, &canonical_request),
    );
    (expected == provided)
        .then_some(())
        .ok_or_else(|| "header signature mismatch".to_owned())
}

/// Percent-decode the only escape our client emits in query values: `%2F`.
fn decode_credential(encoded: &str) -> String {
    encoded.replace("%2F", "/")
}

/// Verify a presigned URL signature for the request's actual method.
fn verify_presigned(parts: &Parts) -> Result<(), String> {
    let host = parts
        .headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .ok_or("missing host")?;
    let query = parts.uri.query().unwrap_or_default();

    let mut params: Vec<(String, String)> = Vec::new();
    let (mut provided, mut credential_enc, mut amz_date) = (None, None, None);
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or("bad query pair")?;
        match key {
            "X-Amz-Signature" => {
                provided = Some(value.to_owned());
                continue;
            }
            "X-Amz-Credential" => credential_enc = Some(value.to_owned()),
            "X-Amz-Date" => amz_date = Some(value.to_owned()),
            _ => {}
        }
        params.push((key.to_owned(), value.to_owned()));
    }
    let provided = provided.ok_or("missing X-Amz-Signature")?;
    let credential = decode_credential(&credential_enc.ok_or("missing credential")?);
    let amz_date = amz_date.ok_or("missing X-Amz-Date")?;
    let (date_stamp, region, service) = parse_scope(&credential)?;

    params.sort();
    let canonical_query = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_request = format!(
        "{}\n{}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD",
        parts.method.as_str(),
        parts.uri.path(),
    );
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let expected = compute_signature(
        &date_stamp,
        &region,
        &service,
        &string_to_sign(&amz_date, &scope, &canonical_request),
    );
    (expected == provided)
        .then_some(())
        .ok_or_else(|| "presigned signature mismatch".to_owned())
}

// ---------------------------------------------------------------------------
// Fake S3 server
// ---------------------------------------------------------------------------

type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;

fn handle(parts: &Parts, body: &Bytes, store: &Store) -> Response<Full<Bytes>> {
    let is_presigned = parts
        .uri
        .query()
        .is_some_and(|query| query.contains("X-Amz-Signature="));
    let verified = if is_presigned {
        verify_presigned(parts)
    } else {
        verify_header_auth(parts, body)
    };
    if let Err(reason) = verified {
        return text(
            StatusCode::FORBIDDEN,
            format!("SignatureDoesNotMatch: {reason}"),
        );
    }

    let key = parts.uri.path().to_owned();
    match parts.method.as_str() {
        "PUT" => {
            store.lock().unwrap().insert(key, body.to_vec());
            empty(StatusCode::OK)
        }
        "GET" => match store.lock().unwrap().get(&key) {
            Some(bytes) => Response::builder()
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from(bytes.clone())))
                .unwrap(),
            None => text(StatusCode::NOT_FOUND, "NoSuchKey".to_owned()),
        },
        "HEAD" => {
            if store.lock().unwrap().contains_key(&key) {
                empty(StatusCode::OK)
            } else {
                empty(StatusCode::NOT_FOUND)
            }
        }
        "DELETE" => {
            store.lock().unwrap().remove(&key);
            empty(StatusCode::NO_CONTENT)
        }
        other => text(StatusCode::METHOD_NOT_ALLOWED, format!("method {other}")),
    }
}

fn text(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

async fn spawn_fake_s3() -> (SocketAddr, Store) {
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("local addr");
    let server_store = Arc::clone(&store);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let store = Arc::clone(&server_store);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let store = Arc::clone(&store);
                        async move {
                            let (parts, body) = request.into_parts();
                            let bytes = body.collect().await.expect("body").to_bytes();
                            Ok::<_, Infallible>(handle(&parts, &bytes, &store))
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (address, store)
}

fn disk_for(address: SocketAddr) -> S3Disk {
    let config = S3Config::new(
        format!("http://{address}"),
        REGION,
        BUCKET,
        ACCESS_KEY,
        SECRET_KEY,
    )
    .with_addressing(Addressing::Path);
    S3Disk::new(config).expect("disk")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_get_delete_round_trip_with_real_sigv4() {
    let (address, _store) = spawn_fake_s3().await;
    let disk = disk_for(address);

    assert!(!disk.exists("docs/hello.txt").await.unwrap());
    disk.put("docs/hello.txt", Bytes::from_static(b"hello s3"))
        .await
        .expect("put");
    assert!(disk.exists("docs/hello.txt").await.unwrap());
    assert_eq!(
        disk.get("docs/hello.txt").await.expect("get").as_ref(),
        b"hello s3"
    );

    disk.delete("docs/hello.txt").await.expect("delete");
    assert!(!disk.exists("docs/hello.txt").await.unwrap());
    assert!(matches!(
        disk.get("docs/hello.txt").await,
        Err(StorageError::NotFound(_))
    ));
    // Deleting a missing key succeeds (S3 semantics).
    disk.delete("docs/hello.txt")
        .await
        .expect("idempotent delete");
}

#[tokio::test]
async fn get_missing_key_is_not_found() {
    let (address, _store) = spawn_fake_s3().await;
    let disk = disk_for(address);
    assert!(matches!(
        disk.get("missing.bin").await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn presigned_get_url_is_verifiable_by_the_server() {
    let (address, _store) = spawn_fake_s3().await;
    let disk = disk_for(address);
    disk.put("covers/a.png", Bytes::from_static(b"PNGDATA"))
        .await
        .expect("put");

    let url = disk
        .presigned_get_url("covers/a.png", Duration::from_mins(10))
        .expect("presign get");

    let http = HyperS3Http::new();
    let response = http
        .send(S3Request {
            method: http::Method::GET,
            url,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("fetch");
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body.as_ref(), b"PNGDATA");
}

#[tokio::test]
async fn presigned_put_url_uploads_then_reads_back() {
    let (address, _store) = spawn_fake_s3().await;
    let disk = disk_for(address);

    let url = disk
        .presigned_put_url("uploads/direct.bin", Duration::from_mins(10))
        .expect("presign put");

    let http = HyperS3Http::new();
    let response = http
        .send(S3Request {
            method: http::Method::PUT,
            url,
            headers: Vec::new(),
            body: Bytes::from_static(b"browser-upload"),
        })
        .await
        .expect("upload");
    assert_eq!(response.status, StatusCode::OK);

    assert_eq!(
        disk.get("uploads/direct.bin").await.expect("get").as_ref(),
        b"browser-upload"
    );
}

#[tokio::test]
async fn tampered_presigned_signature_is_rejected() {
    let (address, _store) = spawn_fake_s3().await;
    let disk = disk_for(address);
    disk.put("covers/b.png", Bytes::from_static(b"data"))
        .await
        .expect("put");

    let mut url = disk
        .presigned_get_url("covers/b.png", Duration::from_mins(10))
        .expect("presign");
    // Flip the final character of the signature.
    let last = url.pop().unwrap();
    url.push(if last == '0' { '1' } else { '0' });

    let http = HyperS3Http::new();
    let response = http
        .send(S3Request {
            method: http::Method::GET,
            url,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("fetch");
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

/// A transport that corrupts the `Authorization` signature, proving the fake
/// server independently rejects an invalid header-auth signature with 403.
struct TamperAuth(HyperS3Http);

impl S3Http for TamperAuth {
    fn send(&self, mut request: S3Request) -> BoxFuture<Result<S3Response, StorageError>> {
        for (name, value) in &mut request.headers {
            if name.eq_ignore_ascii_case("authorization")
                && let Some(last) = value.pop()
            {
                value.push(if last == '0' { '1' } else { '0' });
            }
        }
        self.0.send(request)
    }
}

#[tokio::test]
async fn tampered_header_signature_is_rejected() {
    let (address, _store) = spawn_fake_s3().await;
    let config = S3Config::new(
        format!("http://{address}"),
        REGION,
        BUCKET,
        ACCESS_KEY,
        SECRET_KEY,
    );
    let disk =
        S3Disk::with_transport(config, Arc::new(TamperAuth(HyperS3Http::new()))).expect("disk");

    let error = disk
        .put("docs/x.txt", Bytes::from_static(b"x"))
        .await
        .expect_err("tampered signature must fail");
    assert!(matches!(error, StorageError::Backend(_)), "got {error:?}");
}
