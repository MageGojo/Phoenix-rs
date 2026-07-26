//! One-tap encrypted transport for the Phoenix page protocol (server half).
//!
//! This module negotiates a per-session AES-256-GCM key with the browser over
//! an ephemeral ECDH-P256 handshake, then seals page-protocol responses into a
//! compact binary frame. It is a defence-in-depth layer *on top of* TLS, not a
//! replacement for it: the browser must be able to decrypt in order to render,
//! so this never hides content from the end user. What it does buy is that the
//! session key is negotiated fresh per page session, never ships in the JS
//! bundle, and lives behind a non-extractable `CryptoKey` on the client — which
//! defeats passive capture from logs, bundles, and naive scrapers.
//!
//! The wire protocol is fixed by the TypeScript client
//! (`packages/phoenix-react/src/secure.ts`); every field name, header, salt,
//! info string, and byte offset below is load-bearing.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use phoenix_http::{
    BoxFuture, Bytes, Handler, HeaderValue, Middleware, Next, PAGE_PROTOCOL_MEDIA_TYPE, Request,
    Response, SECURE_CONTENT_TYPE, SECURE_ENCRYPTED_HEADER, SECURE_HANDSHAKE_PATH,
    SECURE_KEY_HEADER, SECURE_PLAINTEXT_TYPE_HEADER, SECURE_REQUEST_HEADER, StatusCode, header,
};
use ring::{
    agreement, hkdf,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

/// Frame magic marker: ASCII `"PHX1"`.
const FRAME_MAGIC: &[u8; 4] = b"PHX1";
/// Frame format version byte.
const FRAME_VERSION: u8 = 1;
/// Length of the authenticated frame header: `magic`(4) + `version`(1) + `issued_at`(8) + `expires_at`(8).
const FRAME_HEADER_LEN: usize = 21;
/// AES-GCM nonce length.
const NONCE_LEN: usize = 12;
/// AES-GCM authentication tag length.
const TAG_LEN: usize = 16;
/// HKDF `info` binding string shared with the client.
const HKDF_INFO: &[u8] = b"phoenix.secure.session.v1";
/// Length of the random material used to mint a `key_id`.
const KEY_ID_ENTROPY_BYTES: usize = 16;
/// Raw uncompressed EC point length for P-256 (`0x04 || X32 || Y32`).
const P256_UNCOMPRESSED_LEN: usize = 65;
/// Negotiated protocol version.
const PROTOCOL_VERSION: u8 = 1;

/// Which half of the exchange a frame belongs to.
///
/// The label is mixed into the AEAD additional data, so a captured frame from
/// one direction cannot be replayed as the other. Without it, a response frame
/// and a request frame under the same session key are interchangeable
/// ciphertexts, and an attacker could feed a captured response back as a
/// request body and have it authenticate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameDirection {
    /// Client → server (encrypted request body).
    Request,
    /// Server → client (encrypted page-protocol response).
    Response,
}

impl FrameDirection {
    /// The bytes appended to the AAD. Must match the client's `secure.ts`.
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Request => b"req",
            Self::Response => b"res",
        }
    }
}

/// `AAD = frame_header ++ key_id ++ direction`.
fn frame_aad(header: &[u8], key_id: &[u8], direction: FrameDirection) -> Vec<u8> {
    let label = direction.label();
    let mut aad = Vec::with_capacity(header.len() + key_id.len() + label.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(key_id);
    aad.extend_from_slice(label);
    aad
}

/// Errors raised while negotiating or applying the secure transport.
#[derive(Debug, Error)]
pub enum SecureError {
    /// The client public key was not a raw uncompressed P-256 point.
    #[error("client public key is not a raw uncompressed P-256 point")]
    InvalidPublicKey,
    /// ECDH agreement or key derivation failed.
    #[error("ECDH key agreement failed")]
    KeyAgreement,
    /// The frame was truncated, mis-tagged, or otherwise malformed.
    #[error("secure frame is malformed")]
    InvalidFrame,
    /// The frame's declared expiry is in the past.
    #[error("secure frame has expired")]
    Expired,
    /// AEAD authentication failed (tampered ciphertext, AAD, nonce, or key).
    #[error("secure frame failed authentication")]
    AuthenticationFailed,
    /// Frame sealing failed.
    #[error("secure frame sealing failed")]
    SealFailed,
    /// The system clock is before the Unix epoch.
    #[error("system clock is before the Unix epoch")]
    Clock,
}

/// Current Unix time in seconds.
fn unix_now() -> Result<u64, SecureError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|_| SecureError::Clock)
}

/// HKDF output-length key type for `ring::hkdf`.
struct HkdfLen(usize);

impl hkdf::KeyType for HkdfLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// Derive the 32-byte AES-256-GCM session key from ECDH shared material.
///
/// `session_key = HKDF-SHA256(ikm = ecdh_shared_x, salt = key_id, info =
/// "phoenix.secure.session.v1")`. `key_id` is the UTF-8 bytes of the base64url
/// key id string, matching the client's `salt = UTF8(key_id)`.
fn derive_session_key(shared_material: &[u8], key_id: &[u8]) -> Zeroizing<[u8; 32]> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, key_id).extract(shared_material);
    let okm = prk
        .expand(&[HKDF_INFO], HkdfLen(32))
        .expect("HKDF-SHA256 supports a 32-byte output");
    let mut key = Zeroizing::new([0_u8; 32]);
    okm.fill(key.as_mut())
        .expect("HKDF output length matches the buffer");
    key
}

/// Run the server half of the ECDH-P256 handshake.
///
/// Generates a fresh ephemeral key pair, agrees with the client's public key,
/// and derives the session key bound to `key_id`. Returns the server's raw
/// uncompressed public point (65 bytes) and the derived session key.
///
/// # Errors
///
/// Returns [`SecureError::InvalidPublicKey`] when `client_public_raw` is not a
/// 65-byte uncompressed point, or [`SecureError::KeyAgreement`] on any ECDH or
/// key-generation failure.
pub fn server_handshake(
    client_public_raw: &[u8],
    key_id: &[u8],
) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>), SecureError> {
    if client_public_raw.len() != P256_UNCOMPRESSED_LEN || client_public_raw[0] != 0x04 {
        return Err(SecureError::InvalidPublicKey);
    }
    let rng = SystemRandom::new();
    let server_private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| SecureError::KeyAgreement)?;
    let server_public = server_private
        .compute_public_key()
        .map_err(|_| SecureError::KeyAgreement)?;
    let server_public_raw = server_public.as_ref().to_vec();

    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, client_public_raw);
    let session_key = agreement::agree_ephemeral(server_private, &peer, |shared_material| {
        derive_session_key(shared_material, key_id)
    })
    .map_err(|_| SecureError::KeyAgreement)?;

    Ok((server_public_raw, session_key))
}

/// Seal `plaintext` into a Phoenix secure frame.
///
/// Frame layout (big-endian), matching the client `decryptFrame`:
///
/// ```text
/// [0]  magic      "PHX1"          (4)
/// [4]  version    0x01            (1)
/// [5]  issued_at  u64 be          (8)
/// [13] expires_at u64 be          (8)
/// [21] nonce                      (12, unique random per frame)
/// [33] ciphertext || gcm_tag(16)  (rest)
/// ```
///
/// `AAD = frame[0..21] ++ key_id ++ direction` — see [`FrameDirection`].
///
/// # Errors
///
/// Returns [`SecureError::SealFailed`] if AEAD encryption fails.
pub fn seal_frame(
    session_key: &[u8; 32],
    key_id: &[u8],
    plaintext: &[u8],
    issued_at: u64,
    ttl_secs: u64,
    direction: FrameDirection,
) -> Result<Vec<u8>, SecureError> {
    let expires_at = issued_at.saturating_add(ttl_secs);
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + NONCE_LEN + plaintext.len() + TAG_LEN);
    frame.extend_from_slice(FRAME_MAGIC);
    frame.push(FRAME_VERSION);
    frame.extend_from_slice(&issued_at.to_be_bytes());
    frame.extend_from_slice(&expires_at.to_be_bytes());
    debug_assert_eq!(frame.len(), FRAME_HEADER_LEN);

    // A fresh random nonce per frame — never reused for a given key.
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    frame.extend_from_slice(nonce.as_slice());

    let aad = frame_aad(&frame[..FRAME_HEADER_LEN], key_id, direction);

    let cipher = Aes256Gcm::new_from_slice(session_key).map_err(|_| SecureError::SealFailed)?;
    let sealed = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| SecureError::SealFailed)?;
    frame.extend_from_slice(&sealed);
    Ok(frame)
}

/// Open a Phoenix secure frame, verifying magic, version, expiry, and AAD.
///
/// # Errors
///
/// Returns [`SecureError::InvalidFrame`] for a malformed frame,
/// [`SecureError::Expired`] when `expires_at < now`, or
/// [`SecureError::AuthenticationFailed`] when the tag, AAD, nonce, or key does
/// not verify.
pub fn open_frame(
    session_key: &[u8; 32],
    key_id: &[u8],
    frame: &[u8],
    now: u64,
    direction: FrameDirection,
) -> Result<Vec<u8>, SecureError> {
    if frame.len() < FRAME_HEADER_LEN + NONCE_LEN + TAG_LEN {
        return Err(SecureError::InvalidFrame);
    }
    if &frame[0..4] != FRAME_MAGIC || frame[4] != FRAME_VERSION {
        return Err(SecureError::InvalidFrame);
    }
    let expires_at = u64::from_be_bytes(
        frame[13..21]
            .try_into()
            .map_err(|_| SecureError::InvalidFrame)?,
    );
    if expires_at < now {
        return Err(SecureError::Expired);
    }
    let nonce = &frame[FRAME_HEADER_LEN..FRAME_HEADER_LEN + NONCE_LEN];
    let sealed = &frame[FRAME_HEADER_LEN + NONCE_LEN..];

    let aad = frame_aad(&frame[..FRAME_HEADER_LEN], key_id, direction);

    let cipher = Aes256Gcm::new_from_slice(session_key).map_err(|_| SecureError::InvalidFrame)?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: &aad,
            },
        )
        .map_err(|_| SecureError::AuthenticationFailed)
}

/// Handshake request body (`POST /__phoenix/secure/handshake`).
#[derive(Debug, Deserialize)]
struct HandshakeRequest {
    v: u8,
    kex: String,
    hkdf: String,
    aead: String,
    client_public_key: String,
}

/// Handshake response body.
#[derive(Debug, Serialize)]
struct HandshakeResponse {
    v: u8,
    key_id: String,
    server_public_key: String,
    expires_at: u64,
    ttl: u64,
}

/// Configuration for the secure transport. Defaults are safe for production.
#[derive(Clone, Debug)]
pub struct SecureTransportConfig {
    /// How long a negotiated session key remains valid.
    pub session_ttl: Duration,
    /// How long an individual response frame remains valid.
    pub frame_ttl: Duration,
    /// Handshake route path (default `/__phoenix/secure/handshake`).
    pub handshake_path: String,
    /// Maximum accepted handshake request body size, in bytes.
    pub max_handshake_body: usize,
    /// Upper bound on stored sessions; oldest-expiring entries are evicted past it.
    pub max_sessions: usize,
    /// Maximum accepted encrypted **request** frame size, in bytes.
    ///
    /// A frame this layer cannot open is rejected before the handler runs, so
    /// this bounds the work an unauthenticated caller can force. Requests
    /// larger than this are refused with `413`.
    pub max_request_frame: usize,
}

impl Default for SecureTransportConfig {
    fn default() -> Self {
        Self {
            session_ttl: Duration::from_mins(5),
            frame_ttl: Duration::from_mins(1),
            handshake_path: SECURE_HANDSHAKE_PATH.to_owned(),
            max_handshake_body: 2048,
            max_sessions: 100_000,
            // Page-protocol action bodies are small JSON documents; file
            // uploads go through multipart, which this layer does not encrypt.
            max_request_frame: 1024 * 1024,
        }
    }
}

/// A negotiated session key and its expiry.
struct Session {
    key: Zeroizing<[u8; 32]>,
    expires_at: u64,
}

struct Inner {
    sessions: RwLock<HashMap<String, Session>>,
    config: SecureTransportConfig,
}

/// Process-local secure transport: the session-key store plus the handshake
/// handler and the response-encrypting middleware that share it.
///
/// Cloning is cheap (`Arc`) — clone it freely to hand the same store to the
/// handshake route and the encryption layer.
#[derive(Clone)]
pub struct SecureTransport {
    inner: Arc<Inner>,
}

impl SecureTransport {
    /// Create a transport with the given configuration.
    #[must_use]
    pub fn new(config: SecureTransportConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                sessions: RwLock::new(HashMap::new()),
                config,
            }),
        }
    }

    /// The configured handshake route path.
    #[must_use]
    pub fn handshake_path(&self) -> &str {
        &self.inner.config.handshake_path
    }

    /// A [`Handler`] serving the ECDH handshake. Mount it as a `POST` route
    /// **outside** any CSRF scope — it runs before a session exists and is an
    /// idempotent negotiation.
    #[must_use]
    pub fn handshake_handler(&self) -> SecureHandshakeHandler {
        SecureHandshakeHandler {
            transport: self.clone(),
        }
    }

    /// A [`Middleware`] that encrypts plaintext page-protocol responses when the
    /// request carries a valid `X-Phoenix-Secure` / `X-Phoenix-Key` pair.
    #[must_use]
    pub fn layer(&self) -> SecureTransportLayer {
        SecureTransportLayer {
            transport: self.clone(),
        }
    }

    /// Negotiate a session for `client_public_raw`, store it, and return the
    /// handshake response payload.
    fn establish(&self, client_public_raw: &[u8]) -> Result<HandshakeResponse, SecureError> {
        let key_id = mint_key_id()?;
        let (server_public_raw, session_key) =
            server_handshake(client_public_raw, key_id.as_bytes())?;

        let now = unix_now()?;
        let ttl = self.inner.config.session_ttl.as_secs();
        let expires_at = now.saturating_add(ttl);

        {
            let mut sessions = self
                .inner
                .sessions
                .write()
                .map_err(|_| SecureError::KeyAgreement)?;
            prune_expired(&mut sessions, now);
            enforce_capacity(&mut sessions, self.inner.config.max_sessions);
            sessions.insert(
                key_id.clone(),
                Session {
                    key: session_key,
                    expires_at,
                },
            );
        }

        Ok(HandshakeResponse {
            v: PROTOCOL_VERSION,
            key_id,
            server_public_key: URL_SAFE_NO_PAD.encode(&server_public_raw),
            expires_at,
            ttl,
        })
    }

    /// Look up a live session key by `key_id`, or `None` if unknown/expired.
    fn session_key(&self, key_id: &str, now: u64) -> Option<Zeroizing<[u8; 32]>> {
        let sessions = self.inner.sessions.read().ok()?;
        let session = sessions.get(key_id)?;
        if session.expires_at < now {
            return None;
        }
        Some(session.key.clone())
    }

    /// Serve one handshake request.
    fn respond_handshake(&self, request: &Request) -> Response {
        if request.body().len() > self.inner.config.max_handshake_body {
            return handshake_error(StatusCode::PAYLOAD_TOO_LARGE, "handshake body too large");
        }
        let Ok(body) = request.json::<HandshakeRequest>() else {
            return handshake_error(StatusCode::BAD_REQUEST, "invalid handshake body");
        };
        if body.v != PROTOCOL_VERSION
            || body.kex != "ECDH-P256"
            || body.hkdf != "HKDF-SHA256"
            || body.aead != "A256GCM"
        {
            return handshake_error(StatusCode::BAD_REQUEST, "unsupported handshake parameters");
        }
        let Ok(client_public_raw) = URL_SAFE_NO_PAD.decode(body.client_public_key.as_bytes())
        else {
            return handshake_error(StatusCode::BAD_REQUEST, "invalid client public key");
        };
        match self.establish(&client_public_raw) {
            Ok(payload) => match serde_json::to_vec(&payload) {
                Ok(bytes) => handshake_ok(bytes),
                Err(_) => {
                    handshake_error(StatusCode::INTERNAL_SERVER_ERROR, "handshake serialization")
                }
            },
            Err(SecureError::InvalidPublicKey) => {
                handshake_error(StatusCode::BAD_REQUEST, "invalid client public key")
            }
            Err(_) => handshake_error(StatusCode::INTERNAL_SERVER_ERROR, "handshake failed"),
        }
    }

    /// Extract a valid secure directive from request headers, if present.
    fn request_directive(
        &self,
        request: &Request,
        now: u64,
    ) -> Option<(String, Zeroizing<[u8; 32]>)> {
        let wants_secure = request
            .headers()
            .get(SECURE_REQUEST_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "1");
        if !wants_secure {
            return None;
        }
        let key_id = request
            .headers()
            .get(SECURE_KEY_HEADER)
            .and_then(|value| value.to_str().ok())?
            .to_owned();
        let key = self.session_key(&key_id, now)?;
        Some((key_id, key))
    }

    /// Open an encrypted request body in place, restoring the plaintext body
    /// and its original content type.
    ///
    /// Returns `Ok(())` unchanged for any request that is not marked as an
    /// encrypted frame — that path is byte-for-byte identical to running
    /// without this layer. A request that *claims* to be encrypted and cannot
    /// be opened fails closed with an error response; it is never passed to the
    /// handler as ciphertext or as an empty body.
    fn decrypt_request(
        &self,
        request: &mut Request,
        key_id: &str,
        key: &[u8; 32],
        now: u64,
    ) -> Result<(), Box<Response>> {
        if !is_encrypted_request(request) {
            return Ok(());
        }
        if request.body().len() > self.inner.config.max_request_frame {
            return Err(Box::new(secure_request_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "encrypted request body too large",
            )));
        }
        let plaintext = open_frame(
            key,
            key_id.as_bytes(),
            request.body(),
            now,
            FrameDirection::Request,
        )
        .map_err(|error| {
            Box::new(match error {
                SecureError::Expired => {
                    secure_request_error(StatusCode::BAD_REQUEST, "encrypted request expired")
                }
                _ => secure_request_error(
                    StatusCode::BAD_REQUEST,
                    "encrypted request could not be authenticated",
                ),
            })
        })?;

        // Restore the plaintext content type the client declared, then drop the
        // frame markers so nothing downstream mistakes this for ciphertext.
        let content_type = request
            .headers()
            .get(SECURE_PLAINTEXT_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| HeaderValue::from_str(value).ok())
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));
        request.replace_body(Bytes::from(plaintext));
        let headers = request.headers_mut();
        headers.insert(header::CONTENT_TYPE, content_type);
        headers.remove(header::CONTENT_LENGTH);
        headers.remove(SECURE_PLAINTEXT_TYPE_HEADER);
        headers.insert(SECURE_ENCRYPTED_HEADER, HeaderValue::from_static("0"));
        Ok(())
    }

    /// Re-seal a plaintext page-protocol response body into a secure frame.
    /// Any other response (document HTML, already-encrypted, streaming,
    /// non-page) is returned unchanged.
    fn encrypt_page_response(&self, response: Response, key_id: &str, key: &[u8; 32]) -> Response {
        if response.is_streaming() || !is_plaintext_page_protocol(&response) {
            return response;
        }
        let Ok(now) = unix_now() else {
            return response;
        };
        let ttl = self.inner.config.frame_ttl.as_secs();
        let Ok(frame) = seal_frame(
            key,
            key_id.as_bytes(),
            response.body(),
            now,
            ttl,
            FrameDirection::Response,
        ) else {
            return response;
        };

        let status = response.status();
        let mut encrypted = Response::new(status, frame);
        // Preserve the page-protocol caching/vary headers, then flip the
        // content-type and encryption marker.
        *encrypted.headers_mut() = response.headers().clone();
        encrypted.headers_mut().remove(header::CONTENT_LENGTH);
        encrypted.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(SECURE_CONTENT_TYPE),
        );
        encrypted
            .headers_mut()
            .insert(SECURE_ENCRYPTED_HEADER, HeaderValue::from_static("1"));
        encrypted
    }
}

/// True when `request` carries a binary Phoenix secure frame as its body.
fn is_encrypted_request(request: &Request) -> bool {
    let marked = request
        .headers()
        .get(SECURE_ENCRYPTED_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "1");
    if !marked {
        return false;
    }
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with(SECURE_CONTENT_TYPE))
}

/// A plaintext JSON error for a request frame this layer refused to open.
///
/// Deliberately terse: the client learns the request was rejected, not which
/// check failed, so this is not an oracle.
fn secure_request_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| b"{\"error\":\"secure request rejected\"}".to_vec());
    let mut response = Response::new(status, body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// True when `response` is a plaintext page-protocol payload eligible for
/// frame encryption (correct content type and `x-phoenix-encrypted: 0`).
fn is_plaintext_page_protocol(response: &Response) -> bool {
    let is_page = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == PAGE_PROTOCOL_MEDIA_TYPE);
    if !is_page {
        return false;
    }
    response
        .headers()
        .get(SECURE_ENCRYPTED_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| value == "0")
}

/// Mint a random base64url `key_id`.
fn mint_key_id() -> Result<String, SecureError> {
    let mut raw = [0_u8; KEY_ID_ENTROPY_BYTES];
    SystemRandom::new()
        .fill(&mut raw)
        .map_err(|_| SecureError::KeyAgreement)?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

/// Drop sessions whose expiry has passed.
fn prune_expired(sessions: &mut HashMap<String, Session>, now: u64) {
    sessions.retain(|_, session| session.expires_at >= now);
}

/// Bound the store size, evicting the soonest-to-expire entries first.
fn enforce_capacity(sessions: &mut HashMap<String, Session>, max_sessions: usize) {
    if max_sessions == 0 || sessions.len() < max_sessions {
        return;
    }
    let overflow = sessions.len() + 1 - max_sessions;
    let mut by_expiry: Vec<(String, u64)> = sessions
        .iter()
        .map(|(id, session)| (id.clone(), session.expires_at))
        .collect();
    by_expiry.sort_unstable_by_key(|(_, expires_at)| *expires_at);
    for (id, _) in by_expiry.into_iter().take(overflow) {
        sessions.remove(&id);
    }
}

/// Build a `200 OK` handshake response (JSON, never cached).
fn handshake_ok(body: Vec<u8>) -> Response {
    let mut response = Response::new(StatusCode::OK, body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Build a JSON error handshake response.
fn handshake_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| b"{\"error\":\"handshake failed\"}".to_vec());
    let mut response = Response::new(status, body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// [`Handler`] for the ECDH handshake endpoint.
pub struct SecureHandshakeHandler {
    transport: SecureTransport,
}

impl Handler for SecureHandshakeHandler {
    fn call(&self, request: Request) -> BoxFuture<Response> {
        let transport = self.transport.clone();
        Box::pin(async move { transport.respond_handshake(&request) })
    }
}

/// [`Middleware`] that encrypts page-protocol responses for secure sessions.
pub struct SecureTransportLayer {
    transport: SecureTransport,
}

impl Middleware for SecureTransportLayer {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let transport = self.transport.clone();
        let now = unix_now().ok();
        let directive = now.and_then(|now| transport.request_directive(&request, now));

        // Open an encrypted request body before the handler runs, so extractors
        // see ordinary plaintext. Without a live session there is nothing to
        // open with: a request that claims to be encrypted is rejected rather
        // than handed on as ciphertext.
        if let Some((key_id, key)) = &directive {
            let Some(now) = now else {
                return Box::pin(async {
                    secure_request_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "server clock unavailable",
                    )
                });
            };
            if let Err(rejection) = transport.decrypt_request(&mut request, key_id, key, now) {
                return Box::pin(async move { *rejection });
            }
        } else if is_encrypted_request(&request) {
            return Box::pin(async {
                secure_request_error(
                    StatusCode::BAD_REQUEST,
                    "encrypted request without a live session",
                )
            });
        }

        Box::pin(async move {
            let response = next.run(request).await;
            match directive {
                Some((key_id, key)) => transport.encrypt_page_response(response, &key_id, &key),
                None => response,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent client-side ECDH+HKDF, mirroring `secure.ts`, used to prove
    /// interoperability against the server derivation.
    fn client_derive(server_public_raw: &[u8], key_id: &[u8]) -> (Vec<u8>, Zeroizing<[u8; 32]>) {
        let rng = SystemRandom::new();
        let client_private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let client_public = client_private
            .compute_public_key()
            .unwrap()
            .as_ref()
            .to_vec();
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, server_public_raw);
        let key = agreement::agree_ephemeral(client_private, &peer, |shared| {
            derive_session_key(shared, key_id)
        })
        .unwrap();
        (client_public, key)
    }

    #[test]
    fn ecdh_hkdf_agrees_on_both_sides() {
        // Client generates its key pair first (as the browser does).
        let rng = SystemRandom::new();
        let client_private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let client_public = client_private
            .compute_public_key()
            .unwrap()
            .as_ref()
            .to_vec();
        assert_eq!(client_public.len(), P256_UNCOMPRESSED_LEN);
        assert_eq!(client_public[0], 0x04);

        let key_id = b"test-key-id";
        let (server_public, server_key) = server_handshake(&client_public, key_id).unwrap();
        assert_eq!(server_public.len(), P256_UNCOMPRESSED_LEN);

        // Client completes agreement against the server public key.
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &server_public);
        let client_key = agreement::agree_ephemeral(client_private, &peer, |shared| {
            derive_session_key(shared, key_id)
        })
        .unwrap();
        assert_eq!(server_key.as_ref(), client_key.as_ref());
    }

    #[test]
    fn frame_byte_layout_is_exact() {
        let key = [7_u8; 32];
        let key_id = b"kid-123";
        let frame = seal_frame(
            &key,
            key_id,
            b"hello world",
            1_000,
            60,
            FrameDirection::Response,
        )
        .unwrap();

        assert_eq!(&frame[0..4], b"PHX1", "magic");
        assert_eq!(frame[4], 1, "version");
        assert_eq!(u64::from_be_bytes(frame[5..13].try_into().unwrap()), 1_000);
        assert_eq!(u64::from_be_bytes(frame[13..21].try_into().unwrap()), 1_060);
        assert_eq!(frame.len(), 21 + 12 + "hello world".len() + 16);

        let opened = open_frame(&key, key_id, &frame, 1_000, FrameDirection::Response).unwrap();
        assert_eq!(opened, b"hello world");
    }

    #[test]
    fn frame_roundtrip_and_negative_cases() {
        let key = [9_u8; 32];
        let key_id = b"abc";
        let frame = seal_frame(&key, key_id, b"secret", 500, 60, FrameDirection::Response).unwrap();

        // Correct open.
        assert_eq!(
            open_frame(&key, key_id, &frame, 500, FrameDirection::Response).unwrap(),
            b"secret"
        );

        // Expired frame.
        assert!(matches!(
            open_frame(&key, key_id, &frame, 10_000, FrameDirection::Response),
            Err(SecureError::Expired)
        ));

        // Wrong key.
        assert!(matches!(
            open_frame(&[1_u8; 32], key_id, &frame, 500, FrameDirection::Response),
            Err(SecureError::AuthenticationFailed)
        ));

        // Wrong key_id (AAD mismatch).
        assert!(matches!(
            open_frame(&key, b"other", &frame, 500, FrameDirection::Response),
            Err(SecureError::AuthenticationFailed)
        ));

        // Tampered ciphertext.
        let mut tampered = frame.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(matches!(
            open_frame(&key, key_id, &tampered, 500, FrameDirection::Response),
            Err(SecureError::AuthenticationFailed)
        ));

        // Tampered header (AAD covers it) — flip the version-adjacent issued_at.
        let mut header_tampered = frame.clone();
        header_tampered[5] ^= 0x01;
        assert!(matches!(
            open_frame(
                &key,
                key_id,
                &header_tampered,
                500,
                FrameDirection::Response
            ),
            Err(SecureError::AuthenticationFailed)
        ));

        // Bad magic.
        let mut bad_magic = frame.clone();
        bad_magic[0] = b'X';
        assert!(matches!(
            open_frame(&key, key_id, &bad_magic, 500, FrameDirection::Response),
            Err(SecureError::InvalidFrame)
        ));

        // Truncated.
        assert!(matches!(
            open_frame(&key, key_id, &frame[..10], 500, FrameDirection::Response),
            Err(SecureError::InvalidFrame)
        ));
    }

    #[test]
    fn full_interop_handshake_then_frame() {
        // Server negotiates against a client public key derived independently.
        let key_id = b"interop-kid";
        // Bootstrap a server public key, then have the "client" re-derive.
        let rng = SystemRandom::new();
        let boot_client =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let boot_client_pub = boot_client.compute_public_key().unwrap().as_ref().to_vec();
        let (server_pub, server_key) = server_handshake(&boot_client_pub, key_id).unwrap();

        // Independent client re-derivation against the same server public key.
        let (_client_pub, client_key) = client_derive(&server_pub, key_id);
        // These come from different ECDH pairs, so they must NOT match — this
        // guards against a broken derivation that ignores the peer key.
        assert_ne!(server_key.as_ref(), client_key.as_ref());

        // Now the real interop path: server seals, client opens with the shared key.
        let (server_pub2, shared) = {
            // Client half.
            let client_priv =
                agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
            let client_pub = client_priv.compute_public_key().unwrap().as_ref().to_vec();
            let (spub, skey) = server_handshake(&client_pub, key_id).unwrap();
            let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &spub);
            let ckey =
                agreement::agree_ephemeral(client_priv, &peer, |m| derive_session_key(m, key_id))
                    .unwrap();
            assert_eq!(skey.as_ref(), ckey.as_ref());
            (spub, skey)
        };
        assert_eq!(server_pub2.len(), P256_UNCOMPRESSED_LEN);

        let plaintext = br#"{"component":"home","props":{}}"#;
        let frame =
            seal_frame(&shared, key_id, plaintext, 42, 60, FrameDirection::Response).unwrap();
        let opened = open_frame(&shared, key_id, &frame, 42, FrameDirection::Response).unwrap();
        assert_eq!(opened, plaintext);
    }

    // ---- End-to-end through the router + middleware -----------------------

    use phoenix_http::{HeaderMap, Method, Uri};
    use phoenix_routing::Routes;

    const PAGE_ENVELOPE: &[u8] = br#"{"component":"dashboard","props":{"secret":42}}"#;

    /// A plaintext page-protocol handler, matching what `phoenix-view` emits.
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

    fn json_request(path: &str, body: Vec<u8>) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Request::from_parts(
            Method::POST,
            path.parse::<Uri>().unwrap(),
            headers,
            body.into(),
        )
    }

    #[tokio::test]
    async fn handshake_then_encrypted_page_end_to_end() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = Routes::new()
            .post(
                transport.handshake_path().to_owned(),
                transport.handshake_handler(),
            )
            .get("/dashboard", page_handler)
            .with_middleware(transport.layer())
            .build()
            .unwrap();

        // 1. Client generates its ECDH key pair (as the browser does).
        let rng = SystemRandom::new();
        let client_private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let client_public = client_private
            .compute_public_key()
            .unwrap()
            .as_ref()
            .to_vec();

        // 2. Handshake request through the router.
        let body = serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "kex": "ECDH-P256",
            "hkdf": "HKDF-SHA256",
            "aead": "A256GCM",
            "client_public_key": URL_SAFE_NO_PAD.encode(&client_public),
        }))
        .unwrap();
        let response = router
            .handle(json_request(transport.handshake_path(), body))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        let key_id = payload["key_id"].as_str().unwrap().to_owned();
        let server_public = URL_SAFE_NO_PAD
            .decode(payload["server_public_key"].as_str().unwrap())
            .unwrap();
        assert_eq!(payload["v"].as_u64(), Some(1));

        // 3. Client re-derives the shared session key.
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &server_public);
        let client_key: Zeroizing<[u8; 32]> =
            agreement::agree_ephemeral(client_private, &peer, |m| {
                derive_session_key(m, key_id.as_bytes())
            })
            .unwrap();

        // 4. Secure page request -> expect a binary frame.
        let mut headers = HeaderMap::new();
        headers.insert(SECURE_REQUEST_HEADER, HeaderValue::from_static("1"));
        headers.insert(SECURE_KEY_HEADER, HeaderValue::from_str(&key_id).unwrap());
        let page = Request::from_parts(
            Method::GET,
            "/dashboard".parse::<Uri>().unwrap(),
            headers,
            Vec::new().into(),
        );
        let secure = router.handle(page).await;
        assert_eq!(
            secure.headers()[header::CONTENT_TYPE],
            SECURE_CONTENT_TYPE,
            "secure page response is a binary frame"
        );
        assert_eq!(secure.headers()[SECURE_ENCRYPTED_HEADER], "1");

        // 5. Client opens the frame with the negotiated key.
        let now = unix_now().unwrap();
        let opened = open_frame(
            &client_key,
            key_id.as_bytes(),
            secure.body(),
            now,
            FrameDirection::Response,
        )
        .unwrap();
        assert_eq!(opened, PAGE_ENVELOPE);

        // 6. Unknown key_id -> transparent plaintext fallback.
        let mut bad_headers = HeaderMap::new();
        bad_headers.insert(SECURE_REQUEST_HEADER, HeaderValue::from_static("1"));
        bad_headers.insert(
            SECURE_KEY_HEADER,
            HeaderValue::from_static("not-a-real-key"),
        );
        let fallback = router
            .handle(Request::from_parts(
                Method::GET,
                "/dashboard".parse::<Uri>().unwrap(),
                bad_headers,
                Vec::new().into(),
            ))
            .await;
        assert_eq!(
            fallback.headers()[header::CONTENT_TYPE],
            PAGE_PROTOCOL_MEDIA_TYPE,
            "unknown key falls back to plaintext"
        );
        assert_eq!(fallback.headers()[SECURE_ENCRYPTED_HEADER], "0");
        assert_eq!(fallback.body().as_ref(), PAGE_ENVELOPE);
    }

    /// Handshake through `router` and return the negotiated `(key_id, key)`.
    async fn negotiate(
        router: &phoenix_routing::Router,
        transport: &SecureTransport,
    ) -> (String, Zeroizing<[u8; 32]>) {
        let rng = SystemRandom::new();
        let client_private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let client_public = client_private
            .compute_public_key()
            .unwrap()
            .as_ref()
            .to_vec();
        let body = serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "kex": "ECDH-P256",
            "hkdf": "HKDF-SHA256",
            "aead": "A256GCM",
            "client_public_key": URL_SAFE_NO_PAD.encode(&client_public),
        }))
        .unwrap();
        let response = router
            .handle(json_request(transport.handshake_path(), body))
            .await;
        let payload: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        let key_id = payload["key_id"].as_str().unwrap().to_owned();
        let server_public = URL_SAFE_NO_PAD
            .decode(payload["server_public_key"].as_str().unwrap())
            .unwrap();
        let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &server_public);
        let key = agreement::agree_ephemeral(client_private, &peer, |m| {
            derive_session_key(m, key_id.as_bytes())
        })
        .unwrap();
        (key_id, key)
    }

    /// An echo handler that reports exactly what the handler layer received.
    async fn echo_handler(request: Request) -> Response {
        let seen = serde_json::json!({
            "body": String::from_utf8_lossy(request.body()),
            "content_type": request
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
        });
        let mut response = Response::new(StatusCode::OK, serde_json::to_vec(&seen).unwrap());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    }

    fn echo_router(transport: &SecureTransport) -> phoenix_routing::Router {
        Routes::new()
            .post(
                transport.handshake_path().to_owned(),
                transport.handshake_handler(),
            )
            .post("/actions/store", echo_handler)
            .with_middleware(transport.layer())
            .build()
            .unwrap()
    }

    /// Build a request whose body is a sealed frame, as the client sends it.
    fn secure_request(key_id: &str, frame: Vec<u8>) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert(SECURE_REQUEST_HEADER, HeaderValue::from_static("1"));
        headers.insert(SECURE_KEY_HEADER, HeaderValue::from_str(key_id).unwrap());
        headers.insert(SECURE_ENCRYPTED_HEADER, HeaderValue::from_static("1"));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(SECURE_CONTENT_TYPE),
        );
        Request::from_parts(
            Method::POST,
            "/actions/store".parse::<Uri>().unwrap(),
            headers,
            frame.into(),
        )
    }

    #[tokio::test]
    async fn encrypted_request_body_reaches_the_handler_as_plaintext() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = echo_router(&transport);
        let (key_id, key) = negotiate(&router, &transport).await;

        let plaintext = br#"{"title":"secret draft"}"#;
        let now = unix_now().unwrap();
        let frame = seal_frame(
            &key,
            key_id.as_bytes(),
            plaintext,
            now,
            60,
            FrameDirection::Request,
        )
        .unwrap();

        let response = router.handle(secure_request(&key_id, frame)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let seen: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(seen["body"], r#"{"title":"secret draft"}"#);
        assert_eq!(
            seen["content_type"], "application/json",
            "the handler sees the plaintext content type, not the frame's"
        );
    }

    #[tokio::test]
    async fn a_plaintext_request_is_untouched_byte_for_byte() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = echo_router(&transport);
        let (key_id, _key) = negotiate(&router, &transport).await;

        // A live session, but this request is not marked as an encrypted frame.
        let mut headers = HeaderMap::new();
        headers.insert(SECURE_REQUEST_HEADER, HeaderValue::from_static("1"));
        headers.insert(SECURE_KEY_HEADER, HeaderValue::from_str(&key_id).unwrap());
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let request = Request::from_parts(
            Method::POST,
            "/actions/store".parse::<Uri>().unwrap(),
            headers,
            br#"{"plain":true}"#.to_vec().into(),
        );

        let response = router.handle(request).await;
        let seen: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(seen["body"], r#"{"plain":true}"#);
        assert_eq!(seen["content_type"], "application/json");
    }

    #[tokio::test]
    async fn the_plaintext_content_type_is_restored_when_declared() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = echo_router(&transport);
        let (key_id, key) = negotiate(&router, &transport).await;

        let now = unix_now().unwrap();
        let frame = seal_frame(
            &key,
            key_id.as_bytes(),
            b"a=1&b=2",
            now,
            60,
            FrameDirection::Request,
        )
        .unwrap();
        let mut request = secure_request(&key_id, frame);
        request.headers_mut().insert(
            SECURE_PLAINTEXT_TYPE_HEADER,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let response = router.handle(request).await;
        let seen: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(seen["body"], "a=1&b=2");
        assert_eq!(seen["content_type"], "application/x-www-form-urlencoded");
    }

    #[tokio::test]
    async fn unopenable_request_frames_fail_closed() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = echo_router(&transport);
        let (key_id, key) = negotiate(&router, &transport).await;
        let now = unix_now().unwrap();
        let good = |ttl: u64| {
            seal_frame(
                &key,
                key_id.as_bytes(),
                b"{}",
                now,
                ttl,
                FrameDirection::Request,
            )
            .unwrap()
        };

        // Tampered ciphertext.
        let mut tampered = good(60);
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        // Tampered AAD (the frame header is authenticated).
        let mut header_tampered = good(60);
        header_tampered[5] ^= 0x01;
        // Tampered nonce.
        let mut nonce_tampered = good(60);
        nonce_tampered[FRAME_HEADER_LEN] ^= 0x01;
        // Sealed under a different key.
        let wrong_key = seal_frame(
            &[3_u8; 32],
            key_id.as_bytes(),
            b"{}",
            now,
            60,
            FrameDirection::Request,
        )
        .unwrap();
        // A frame sealed for the *response* direction, replayed as a request:
        // same key, same key_id, still refused.
        let wrong_direction = seal_frame(
            &key,
            key_id.as_bytes(),
            b"{}",
            now,
            60,
            FrameDirection::Response,
        )
        .unwrap();

        for (label, frame) in [
            ("tampered ciphertext", tampered),
            ("tampered header", header_tampered),
            ("tampered nonce", nonce_tampered),
            ("wrong key", wrong_key),
            ("wrong direction", wrong_direction),
            ("truncated", good(60)[..20].to_vec()),
            ("not a frame at all", b"{\"title\":\"plain\"}".to_vec()),
        ] {
            let response = router.handle(secure_request(&key_id, frame)).await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{label} must be refused"
            );
        }

        // Expired frame: sealed with a ttl of 0, so it is already stale.
        let expired = seal_frame(
            &key,
            key_id.as_bytes(),
            b"{}",
            now.saturating_sub(120),
            1,
            FrameDirection::Request,
        )
        .unwrap();
        let response = router.handle(secure_request(&key_id, expired)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_encrypted_request_without_a_session_is_refused() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let router = echo_router(&transport);

        // No handshake at all: the key id is unknown, so there is nothing to
        // decrypt with. Passing ciphertext to the handler would be worse than
        // refusing, so this fails closed rather than falling back.
        let response = router
            .handle(secure_request("never-negotiated", vec![0_u8; 64]))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn oversized_request_frames_are_refused_before_decryption() {
        let transport = SecureTransport::new(SecureTransportConfig {
            max_request_frame: 64,
            ..SecureTransportConfig::default()
        });
        let router = echo_router(&transport);
        let (key_id, key) = negotiate(&router, &transport).await;

        let now = unix_now().unwrap();
        let frame = seal_frame(
            &key,
            key_id.as_bytes(),
            &vec![b'x'; 512],
            now,
            60,
            FrameDirection::Request,
        )
        .unwrap();
        let response = router.handle(secure_request(&key_id, frame)).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn handshake_rejects_bad_parameters() {
        let transport = SecureTransport::new(SecureTransportConfig::default());
        let body = serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "kex": "ECDH-P384",
            "hkdf": "HKDF-SHA256",
            "aead": "A256GCM",
            "client_public_key": "AAAA",
        }))
        .unwrap();
        let response = transport
            .handshake_handler()
            .call(json_request(transport.handshake_path(), body))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
