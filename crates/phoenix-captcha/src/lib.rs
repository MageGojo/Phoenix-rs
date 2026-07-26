//! SVG image captcha Feature for Phoenix-rs.
//!
//! Generates distorted-text captchas as pure-Rust SVG (no image libraries or C
//! dependencies) and verifies them exactly once with a constant-time
//! comparison. Install with [`phoenix_plugin::FeatureSet::plugin`] like any
//! other Feature.
//!
//! Two storage flows ship, and they share the generator, the hashing, and the
//! one-time-use semantics:
//!
//! - **Session flow** ([`Captcha::issue`] / [`Captcha::verify`]): the hashed
//!   answer lives in the server-side session. Needs a session cookie; the
//!   challenge lives as long as the session does.
//! - **Store flow** ([`Captcha::issue_stored`] / [`Captcha::verify_stored`]):
//!   the hashed answer lives in a [`CaptchaStore`] under an opaque challenge id
//!   the client echoes back. Works without a session (stateless API clients),
//!   expires on its own [`CaptchaConfig::ttl`], and — with
//!   [`DbCaptchaStore`] — keeps one-time use correct across instances.
//!
//! A captcha is low-cost friction, not a security boundary: always combine it
//! with `phoenix_security::RateLimit`. See `docs/CAPTCHA.md`.

mod db_store;
mod store;
mod svg;

use std::{
    borrow::Cow,
    ops::Deref,
    sync::Arc,
    time::{Duration, SystemTime},
};

use phoenix_database::Migration;
use phoenix_http::{
    Bytes, FromRequest, HeaderValue, IntoResponse, Json, Request, Response, StatusCode, header,
};
use phoenix_plugin::{Capability, Plugin};
use phoenix_routing::Routes;
use phoenix_security::Session;
use phoenix_validation::{Rule, RuleContext};
use rand::RngExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use db_store::{CAPTCHAS_TABLE, CaptchaRow, DbCaptchaStore};
pub use store::{CaptchaStore, CaptchaStoreError, MemoryCaptchaStore, StoredChallenge};

/// Default charset without the easily confused glyphs `0`, `O`, `1`, `l`, `I`.
pub const DEFAULT_CHARSET: &str = "23456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

/// Session key holding the hashed answer of the pending challenge.
pub const DEFAULT_SESSION_KEY: &str = "_captcha";

/// Default lifetime of a store-backed challenge ([`CaptchaConfig::ttl`]).
pub const DEFAULT_TTL: Duration = Duration::from_mins(5);

/// Longest accepted user input; anything longer fails verification outright.
const MAX_INPUT_LENGTH: usize = 128;

/// Longest accepted challenge id; anything longer is rejected before it reaches
/// the store (ids we mint are always [`CHALLENGE_ID_LENGTH`] hex characters).
const MAX_CHALLENGE_ID_LENGTH: usize = 128;

/// Length of a minted challenge id: 128 bits of CSPRNG entropy as hex.
const CHALLENGE_ID_LENGTH: usize = 32;

/// Tunable captcha generation settings.
#[derive(Clone, Debug)]
pub struct CaptchaConfig {
    /// Candidate characters; must be non-empty ASCII alphanumeric.
    pub charset: String,
    /// Number of characters per challenge (1–16, default 5).
    pub length: u32,
    /// SVG canvas width in pixels.
    pub width: u32,
    /// SVG canvas height in pixels.
    pub height: u32,
    /// Number of interference curves drawn across the canvas.
    pub noise_curves: u32,
    /// Number of noise dots scattered over the canvas.
    pub noise_dots: u32,
    /// Session key under which the hashed answer is stored.
    pub session_key: String,
    /// Lifetime of a store-backed challenge (1s–1d). The session flow ignores
    /// this: there, the challenge lives as long as the session.
    pub ttl: Duration,
}

impl Default for CaptchaConfig {
    fn default() -> Self {
        Self {
            charset: DEFAULT_CHARSET.to_owned(),
            length: 5,
            width: 160,
            height: 60,
            noise_curves: 3,
            noise_dots: 28,
            session_key: DEFAULT_SESSION_KEY.to_owned(),
            ttl: DEFAULT_TTL,
        }
    }
}

/// Captcha configuration error (fail closed).
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CaptchaError {
    #[error("captcha charset must not be empty")]
    EmptyCharset,
    #[error("captcha charset must contain only ASCII alphanumeric characters")]
    NonAlphanumericCharset,
    #[error("captcha length must be between 1 and 16")]
    InvalidLength,
    #[error("captcha canvas must be at least 60x24 pixels")]
    InvalidDimensions,
    #[error("captcha session key must not be empty")]
    EmptySessionKey,
    #[error("captcha ttl must be between 1 second and 1 day")]
    InvalidTtl,
}

/// One generated challenge: the plaintext answer and its rendered SVG.
#[derive(Clone, Debug)]
pub struct Challenge {
    /// The characters shown in the image (original case).
    pub answer: String,
    /// Self-contained `image/svg+xml` document.
    pub svg: String,
}

/// Captcha generator and session-backed verifier. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Captcha {
    config: Arc<CaptchaConfig>,
}

impl Default for Captcha {
    fn default() -> Self {
        Self {
            config: Arc::new(CaptchaConfig::default()),
        }
    }
}

impl Captcha {
    /// Validate a configuration and build a captcha service.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaError`] when the charset, length, canvas size, or
    /// session key is invalid.
    pub fn new(config: CaptchaConfig) -> Result<Self, CaptchaError> {
        if config.charset.is_empty() {
            return Err(CaptchaError::EmptyCharset);
        }
        if !config.charset.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return Err(CaptchaError::NonAlphanumericCharset);
        }
        if !(1..=16).contains(&config.length) {
            return Err(CaptchaError::InvalidLength);
        }
        if config.width < 60 || config.height < 24 {
            return Err(CaptchaError::InvalidDimensions);
        }
        if config.session_key.trim().is_empty() {
            return Err(CaptchaError::EmptySessionKey);
        }
        if config.ttl < Duration::from_secs(1) || config.ttl > Duration::from_hours(24) {
            return Err(CaptchaError::InvalidTtl);
        }
        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// The validated configuration backing this service.
    #[must_use]
    pub fn config(&self) -> &CaptchaConfig {
        &self.config
    }

    /// Generate a fresh challenge without touching any session.
    #[must_use]
    pub fn generate(&self) -> Challenge {
        let glyphs: Vec<char> = self.config.charset.chars().collect();
        let mut rng = rand::rng();
        let answer: String = (0..self.config.length)
            .map(|_| glyphs[rng.random_range(0..glyphs.len())])
            .collect();
        let svg = svg::render(&self.config, &answer);
        Challenge { answer, svg }
    }

    /// Store the hashed (lowercased) answer of a challenge in the session.
    ///
    /// Only the SHA-256 hex digest is stored — never the plaintext answer.
    /// Storing a new challenge replaces any pending one.
    pub fn store(&self, session: &Session, challenge: &Challenge) {
        session.put(
            self.config.session_key.clone(),
            Value::String(hash_answer(&challenge.answer)),
        );
    }

    /// Generate a challenge, store its hashed answer, and return the SVG
    /// response (`image/svg+xml` with `no-store` caching headers).
    #[must_use]
    pub fn issue(&self, session: &Session) -> Response {
        let challenge = self.generate();
        self.store(session, &challenge);
        svg_response(challenge.svg)
    }

    /// Verify user input against the pending challenge, consuming it.
    ///
    /// The stored challenge is removed **before** comparison, so a challenge
    /// can be attempted exactly once regardless of the outcome. Comparison is
    /// case-insensitive and constant-time over the stored hash.
    #[must_use]
    pub fn verify(&self, session: &Session, input: &str) -> bool {
        verify_with_key(session, &self.config.session_key, input)
    }

    /// Generate a challenge and persist its hashed answer in `store` under a
    /// fresh, unguessable id — the session-less counterpart of [`Self::issue`].
    ///
    /// The returned [`IssuedChallenge`] carries the id the client must echo
    /// back on submit, the SVG document, and the remaining lifetime. Nothing
    /// about the plaintext answer leaves this call.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaStoreError`] when the store rejects the insert.
    pub async fn issue_stored(
        &self,
        store: &dyn CaptchaStore,
    ) -> Result<IssuedChallenge, CaptchaStoreError> {
        let challenge = self.generate();
        let id = new_challenge_id();
        store
            .insert(StoredChallenge {
                id: id.clone(),
                answer_hash: hash_answer(&challenge.answer),
                expires_at: SystemTime::now() + self.config.ttl,
            })
            .await?;
        Ok(IssuedChallenge {
            id,
            svg: challenge.svg,
            expires_in: self.config.ttl,
        })
    }

    /// Verify `input` against the stored challenge `id`, consuming it.
    ///
    /// Same semantics as [`Self::verify`] — the challenge is claimed before
    /// comparison, so it can be attempted exactly once whatever the outcome,
    /// and comparison is case-insensitive and constant-time over the hash.
    /// Expired challenges verify as `false`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaStoreError`] when the store cannot be reached. Callers
    /// must treat that as a failure (fail closed), not as a passing captcha.
    pub async fn verify_stored(
        &self,
        store: &dyn CaptchaStore,
        id: &str,
        input: &str,
    ) -> Result<bool, CaptchaStoreError> {
        let id = id.trim();
        if id.is_empty() || id.len() > MAX_CHALLENGE_ID_LENGTH {
            return Ok(false);
        }
        let Some(challenge) = store.take(id).await? else {
            return Ok(false);
        };
        let input = input.trim();
        if input.is_empty() || input.len() > MAX_INPUT_LENGTH {
            return Ok(false);
        }
        Ok(constant_time_eq(
            &challenge.answer_hash,
            &hash_answer(input),
        ))
    }
}

/// A challenge issued through a [`CaptchaStore`].
#[derive(Clone, Debug)]
pub struct IssuedChallenge {
    /// Opaque id the client submits alongside the answer.
    pub id: String,
    /// Self-contained `image/svg+xml` document.
    pub svg: String,
    /// How long the challenge stays valid ([`CaptchaConfig::ttl`]).
    pub expires_in: Duration,
}

impl IssuedChallenge {
    /// JSON body served by the `captcha.challenge` route:
    /// `{ "id": …, "svg": …, "expires_in": seconds }`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "svg": self.svg,
            "expires_in": self.expires_in.as_secs(),
        })
    }
}

/// 128 bits of CSPRNG entropy as lowercase hex.
fn new_challenge_id() -> String {
    let id = format!("{:032x}", rand::rng().random::<u128>());
    debug_assert_eq!(id.len(), CHALLENGE_ID_LENGTH);
    id
}

/// Verify and consume the pending challenge stored under `session_key`.
///
/// Returns `false` when no challenge is pending, the input is empty or
/// oversized, or the hashes differ. The stored value is always removed first
/// (one attempt per challenge).
#[must_use]
pub fn verify_with_key(session: &Session, session_key: &str, input: &str) -> bool {
    let Some(stored) = session
        .get(session_key)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
    else {
        return false;
    };
    session.remove(session_key);
    let input = input.trim();
    if input.is_empty() || input.len() > MAX_INPUT_LENGTH {
        return false;
    }
    constant_time_eq(&stored, &hash_answer(input))
}

fn hash_answer(answer: &str) -> String {
    use std::fmt::Write as _;

    let normalized = answer.trim().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    digest
        .iter()
        .fold(String::with_capacity(digest.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn svg_response(svg: String) -> Response {
    let mut response = Response::new(StatusCode::OK, Bytes::from(svg));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn missing_session_response() -> Response {
    Response::text("Session middleware is required").with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

fn store_unavailable_response() -> Response {
    // The store failed, not the user. Never leak backend detail to the client.
    Response::text("Captcha store unavailable").with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

/// The 422 body a failed captcha produces, shaped like `phoenix_validation`
/// field errors so form clients consume it unchanged.
///
/// [`CaptchaProtected`] returns this automatically; handlers doing the
/// session-less [`Captcha::verify_stored`] check themselves should return it so
/// both flows look identical to the frontend.
#[must_use]
pub fn captcha_error_response(field: &str) -> Response {
    Json(json!({
        "message": "The submitted data is invalid.",
        "errors": {
            field: [{
                "rule": "captcha",
                "message": "The captcha is invalid or has expired.",
            }],
        },
    }))
    .into_response()
    .with_status(StatusCode::UNPROCESSABLE_ENTITY)
}

/// Ordered id of the `captchas` table migration.
///
/// Sorts after `phoenix-notify`'s `202607260002` so apps installing several
/// plugins keep a strictly increasing migration list.
pub const CAPTCHAS_MIGRATION_ID: &str = "202607260003";

/// The `captchas` table: one row per pending store-backed challenge.
///
/// Only installed when [`CaptchaFeature::with_store`] is used; the session flow
/// needs no table. SQL targets `SQLite` first (the workspace default) and is
/// accepted by `PostgreSQL`; `MySQL` needs an adjusted `DROP INDEX` (same note
/// as the `payments` and `notifications` migrations).
#[must_use]
pub fn captchas_migration() -> Migration {
    Migration::new(CAPTCHAS_MIGRATION_ID, "create captchas table")
        .up("CREATE TABLE IF NOT EXISTS captchas (\
             id TEXT PRIMARY KEY, \
             answer_hash TEXT NOT NULL, \
             expires_at TEXT NOT NULL)")
        .up("CREATE INDEX IF NOT EXISTS captchas_expires_at ON captchas (expires_at)")
        .down("DROP INDEX IF EXISTS captchas_expires_at")
        .down("DROP TABLE IF EXISTS captchas")
}

/// Installable Feature exposing the captcha routes.
///
/// Install with `FeatureSet::new().plugin(CaptchaFeature::new())?`:
///
/// - `GET /captcha` (route name `captcha.image`) always serves the SVG through
///   the session flow, and needs `SessionMiddleware` mounted.
/// - `GET /captcha/challenge` (route name `captcha.challenge`) is added only by
///   [`Self::with_store`]. It serves `{ id, svg, expires_in }` and needs no
///   session; installing it also contributes the `captchas` migration.
pub struct CaptchaFeature {
    captcha: Captcha,
    path: String,
    challenge_path: String,
    store: Option<Arc<dyn CaptchaStore>>,
}

impl Default for CaptchaFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptchaFeature {
    /// Feature with default configuration serving `GET /captcha`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            captcha: Captcha::default(),
            path: "/captcha".to_owned(),
            challenge_path: "/captcha/challenge".to_owned(),
            store: None,
        }
    }

    /// Feature with a custom configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaError`] when the configuration is invalid.
    pub fn with_config(config: CaptchaConfig) -> Result<Self, CaptchaError> {
        Ok(Self {
            captcha: Captcha::new(config)?,
            ..Self::new()
        })
    }

    /// Serve the captcha image from a different path.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Serve the JSON challenge from a different path.
    #[must_use]
    pub fn challenge_path(mut self, path: impl Into<String>) -> Self {
        self.challenge_path = path.into();
        self
    }

    /// Back the session-less flow with `store`, adding the `captcha.challenge`
    /// route and the `captchas` migration.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn CaptchaStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// A cloneable handle for calling [`Captcha::verify`] /
    /// [`Captcha::verify_stored`] in app handlers.
    #[must_use]
    pub fn captcha(&self) -> Captcha {
        self.captcha.clone()
    }

    /// The configured store, if any — hand it to handlers doing session-less
    /// verification.
    #[must_use]
    pub fn store(&self) -> Option<Arc<dyn CaptchaStore>> {
        self.store.clone()
    }
}

impl Plugin for CaptchaFeature {
    fn name(&self) -> &'static str {
        "captcha"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn capabilities(&self) -> &'static [Capability] {
        if self.store.is_some() {
            &[Capability::Routes, Capability::Migrations]
        } else {
            &[Capability::Routes]
        }
    }

    fn migrations(&self) -> Vec<Migration> {
        // Only the store flow needs a table; the session flow stays schema-free.
        if self.store.is_some() {
            vec![captchas_migration()]
        } else {
            Vec::new()
        }
    }

    fn routes(&self) -> Routes {
        let captcha = self.captcha.clone();
        let routes = Routes::new()
            .get(self.path.clone(), move |request: Request| {
                let captcha = captcha.clone();
                async move {
                    let Some(session) = request.extensions().get::<Session>() else {
                        return missing_session_response();
                    };
                    captcha.issue(session)
                }
            })
            .name("image");

        let Some(store) = self.store.clone() else {
            return routes;
        };
        let captcha = self.captcha.clone();
        routes
            .get(self.challenge_path.clone(), move |_request: Request| {
                let captcha = captcha.clone();
                let store = Arc::clone(&store);
                async move {
                    match captcha.issue_stored(store.as_ref()).await {
                        Ok(issued) => {
                            let mut response = Json(issued.to_json()).into_response();
                            response.headers_mut().insert(
                                header::CACHE_CONTROL,
                                HeaderValue::from_static("no-store, no-cache, must-revalidate"),
                            );
                            response
                        }
                        Err(_) => store_unavailable_response(),
                    }
                }
            })
            .name("challenge")
    }
}

/// Request DTOs carrying a captcha answer field.
///
/// Implement this for your input contract, then wrap the extractor in
/// [`CaptchaProtected`] to verify (and consume) the pending challenge before
/// the handler body runs.
pub trait CaptchaInput {
    /// The user-supplied captcha text.
    fn captcha_input(&self) -> &str;

    /// Field name reported in 422 validation errors.
    #[must_use]
    fn captcha_field() -> &'static str
    where
        Self: Sized,
    {
        "captcha"
    }
}

impl<T: CaptchaInput> CaptchaInput for Json<T> {
    fn captcha_input(&self) -> &str {
        self.0.captcha_input()
    }

    fn captcha_field() -> &'static str {
        T::captcha_field()
    }
}

impl<T: CaptchaInput> CaptchaInput for phoenix_http::Form<T> {
    fn captcha_input(&self) -> &str {
        self.0.captcha_input()
    }

    fn captcha_field() -> &'static str {
        T::captcha_field()
    }
}

impl<E: CaptchaInput> CaptchaInput for phoenix_validation::Validated<E> {
    fn captcha_input(&self) -> &str {
        self.0.captcha_input()
    }

    fn captcha_field() -> &'static str {
        E::captcha_field()
    }
}

/// Extractor wrapper that verifies the captcha under [`DEFAULT_SESSION_KEY`]
/// after the inner extractor (e.g. `Validated<Json<T>>`) succeeds.
///
/// Verification consumes the pending challenge; failure yields a 422 JSON
/// body shaped like `phoenix_validation` field errors. Apps using a custom
/// `session_key` should call [`Captcha::verify`] explicitly instead.
#[derive(Clone, Debug)]
pub struct CaptchaProtected<E>(pub E);

impl<E> Deref for CaptchaProtected<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Rejection for [`CaptchaProtected`].
#[derive(Debug)]
pub enum CaptchaRejection<R> {
    /// The inner extractor failed; its own rejection is returned unchanged.
    Extract(R),
    /// `SessionMiddleware` is not mounted (framework misconfiguration).
    MissingSession,
    /// No pending challenge or the answer did not match.
    Invalid {
        /// Field name used in the 422 error body.
        field: &'static str,
    },
}

impl<R> IntoResponse for CaptchaRejection<R>
where
    R: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            Self::Extract(rejection) => rejection.into_response(),
            Self::MissingSession => missing_session_response(),
            Self::Invalid { field } => captcha_error_response(field),
        }
    }
}

impl<E, T> FromRequest for CaptchaProtected<E>
where
    E: FromRequest + Deref<Target = T>,
    T: CaptchaInput,
{
    type Rejection = CaptchaRejection<E::Rejection>;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        let extracted = E::from_request(request).map_err(CaptchaRejection::Extract)?;
        let Some(session) = request.extensions().get::<Session>() else {
            return Err(CaptchaRejection::MissingSession);
        };
        if !verify_with_key(session, DEFAULT_SESSION_KEY, extracted.captcha_input()) {
            return Err(CaptchaRejection::Invalid {
                field: T::captcha_field(),
            });
        }
        Ok(Self(extracted))
    }
}

/// Format-only validation rule for `phoenix_validation` rule lists.
///
/// Checks presence, exact length, and ASCII-alphanumeric characters. It does
/// **not** (and cannot) check the session-stored answer — that requires the
/// request session, so use [`CaptchaProtected`] or [`Captcha::verify`] in the
/// handler for the actual one-time verification.
#[derive(Clone, Copy, Debug)]
pub struct CaptchaFormat {
    length: u32,
}

/// Build a [`CaptchaFormat`] rule for challenges of `length` characters.
#[must_use]
pub const fn captcha_format(length: u32) -> CaptchaFormat {
    CaptchaFormat { length }
}

impl Rule for CaptchaFormat {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("captcha")
    }

    fn validate(&self, context: RuleContext<'_>) -> Result<(), String> {
        let valid = matches!(context.value, Some(Value::String(value)) if {
            let trimmed = value.trim();
            !trimmed.is_empty()
                && u32::try_from(trimmed.chars().count()) == Ok(self.length)
                && trimmed.chars().all(|ch| ch.is_ascii_alphanumeric())
        });
        if valid {
            Ok(())
        } else {
            Err(format!(
                "The {field} field must be the {length}-character text from the captcha image.",
                field = context.field,
                length = self.length,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use phoenix_http::{HeaderMap, Method, header};
    use phoenix_routing::Routes;
    use phoenix_security::{SessionConfig, SessionMiddleware, SessionStore};
    use serde::Deserialize;

    use super::*;

    async fn harvest_session() -> Session {
        let store = SessionStore::memory(Duration::from_mins(2));
        let slot: Arc<Mutex<Option<Session>>> = Arc::new(Mutex::new(None));
        let capture = Arc::clone(&slot);
        let router = Routes::new()
            .get("/session", move |request: Request| {
                let capture = Arc::clone(&capture);
                async move {
                    *capture.lock().expect("capture slot") =
                        request.extensions().get::<Session>().cloned();
                    "ok".into_response()
                }
            })
            .with_middleware(SessionMiddleware::new(store, SessionConfig::default()))
            .build()
            .expect("router builds");
        let _ = router
            .handle(Request::new(Method::GET, "/session".parse().expect("uri")))
            .await;
        let session = slot.lock().expect("capture slot").take();
        session.expect("session captured")
    }

    #[test]
    fn svg_contains_every_glyph_but_not_the_full_answer() {
        let captcha = Captcha::default();
        let challenge = captcha.generate();
        assert_eq!(challenge.answer.chars().count(), 5);
        assert!(challenge.svg.starts_with("<svg "));
        assert!(challenge.svg.ends_with("</svg>"));
        for glyph in challenge.answer.chars() {
            assert!(
                challenge.svg.contains(&format!(">{glyph}</text>")),
                "glyph {glyph} missing from SVG"
            );
        }
        assert!(
            !challenge.svg.contains(&challenge.answer),
            "full answer must not appear contiguously in the SVG"
        );
        assert_eq!(challenge.svg.matches("<text ").count(), 5);
        assert_eq!(challenge.svg.matches("<path ").count(), 3);
        assert_eq!(challenge.svg.matches("<circle ").count(), 28);
    }

    #[test]
    fn generate_respects_configuration() {
        let captcha = Captcha::new(CaptchaConfig {
            charset: "abc".to_owned(),
            length: 4,
            width: 200,
            height: 80,
            noise_curves: 2,
            noise_dots: 10,
            session_key: "quiz".to_owned(),
            ttl: Duration::from_mins(1),
        })
        .expect("valid config");
        let challenge = captcha.generate();
        assert_eq!(challenge.answer.chars().count(), 4);
        assert!(challenge.answer.chars().all(|ch| "abc".contains(ch)));
        assert!(challenge.svg.contains("viewBox=\"0 0 200 80\""));
        assert_eq!(challenge.svg.matches("<text ").count(), 4);
        assert_eq!(challenge.svg.matches("<path ").count(), 2);
        assert_eq!(challenge.svg.matches("<circle ").count(), 10);
    }

    #[test]
    fn invalid_configurations_fail_closed() {
        let config = |mutate: fn(&mut CaptchaConfig)| {
            let mut config = CaptchaConfig::default();
            mutate(&mut config);
            Captcha::new(config)
        };
        assert_eq!(
            config(|c| c.charset.clear()).err(),
            Some(CaptchaError::EmptyCharset)
        );
        assert_eq!(
            config(|c| c.charset = "ab<svg>".to_owned()).err(),
            Some(CaptchaError::NonAlphanumericCharset)
        );
        assert_eq!(
            config(|c| c.length = 0).err(),
            Some(CaptchaError::InvalidLength)
        );
        assert_eq!(
            config(|c| c.length = 17).err(),
            Some(CaptchaError::InvalidLength)
        );
        assert_eq!(
            config(|c| c.width = 10).err(),
            Some(CaptchaError::InvalidDimensions)
        );
        assert_eq!(
            config(|c| c.session_key = "  ".to_owned()).err(),
            Some(CaptchaError::EmptySessionKey)
        );
        assert_eq!(
            config(|c| c.ttl = Duration::from_millis(500)).err(),
            Some(CaptchaError::InvalidTtl)
        );
        assert_eq!(
            config(|c| c.ttl = Duration::from_hours(24) + Duration::from_secs(1)).err(),
            Some(CaptchaError::InvalidTtl)
        );
    }

    #[tokio::test]
    async fn session_stores_hash_not_plaintext() {
        let session = harvest_session().await;
        let captcha = Captcha::default();
        let challenge = captcha.generate();
        captcha.store(&session, &challenge);
        let stored = session
            .get(DEFAULT_SESSION_KEY)
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("hash stored");
        assert_ne!(stored, challenge.answer);
        assert_ne!(stored.to_lowercase(), challenge.answer.to_lowercase());
        assert_eq!(stored.len(), 64);
        assert!(stored.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn verify_is_case_insensitive_and_single_use() {
        let session = harvest_session().await;
        let captcha = Captcha::default();
        let challenge = captcha.generate();

        captcha.store(&session, &challenge);
        assert!(captcha.verify(&session, &challenge.answer.to_uppercase()));
        assert!(
            !captcha.verify(&session, &challenge.answer),
            "a challenge must be usable exactly once"
        );

        captcha.store(&session, &challenge);
        assert!(captcha.verify(
            &session,
            &format!("  {}  ", challenge.answer.to_lowercase())
        ));

        captcha.store(&session, &challenge);
        assert!(!captcha.verify(&session, "wrong"));
        assert!(
            !captcha.verify(&session, &challenge.answer),
            "a failed attempt must also consume the challenge"
        );

        assert!(!captcha.verify(&session, ""));
    }

    #[tokio::test]
    async fn issue_writes_session_and_returns_svg_response() {
        let session = harvest_session().await;
        let captcha = Captcha::default();
        let response = captcha.issue(&session);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store, no-cache, must-revalidate"
        );
        assert_eq!(response.headers().get(header::PRAGMA).unwrap(), "no-cache");
        let body = std::str::from_utf8(response.body()).expect("utf-8 svg");
        assert!(body.starts_with("<svg "));
        assert!(session.get(DEFAULT_SESSION_KEY).is_some());
    }

    #[tokio::test]
    async fn feature_route_serves_svg_behind_session_middleware() {
        let feature = CaptchaFeature::new();
        let store = SessionStore::memory(Duration::from_mins(2));
        let router = phoenix_plugin::FeatureSet::new()
            .plugin(feature)
            .expect("feature installs")
            .into_routes()
            .with_middleware(SessionMiddleware::new(store, SessionConfig::default()))
            .build()
            .expect("router builds");
        assert_eq!(router.url("captcha.image", &[]).expect("named"), "/captcha");
        let response = router
            .handle(Request::new(Method::GET, "/captcha".parse().expect("uri")))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml; charset=utf-8"
        );
        assert!(
            std::str::from_utf8(response.body())
                .expect("utf-8 svg")
                .starts_with("<svg ")
        );
    }

    #[tokio::test]
    async fn feature_route_requires_session_middleware() {
        let router = phoenix_plugin::FeatureSet::new()
            .plugin(CaptchaFeature::new().path("/kaptcha"))
            .expect("feature installs")
            .into_routes()
            .build()
            .expect("router builds");
        let response = router
            .handle(Request::new(Method::GET, "/kaptcha".parse().expect("uri")))
            .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[derive(Deserialize)]
    struct LoginInput {
        captcha: String,
    }

    impl CaptchaInput for LoginInput {
        fn captcha_input(&self) -> &str {
            &self.captcha
        }
    }

    fn json_request(body: String, session: &Session) -> Request {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let mut request = Request::from_parts(
            Method::POST,
            "/login".parse().expect("uri"),
            headers,
            Bytes::from(body),
        );
        request.extensions_mut().insert(session.clone());
        request
    }

    #[tokio::test]
    async fn captcha_protected_extractor_verifies_once() {
        let session = harvest_session().await;
        let captcha = Captcha::default();
        let challenge = captcha.generate();
        captcha.store(&session, &challenge);

        let body = format!(r#"{{"captcha":"{}"}}"#, challenge.answer);
        let request = json_request(body.clone(), &session);
        let extracted =
            CaptchaProtected::<Json<LoginInput>>::from_request(&request).expect("captcha verifies");
        assert_eq!(extracted.0.0.captcha, challenge.answer);

        // The challenge was consumed: the same submission must now fail as 422.
        let request = json_request(body, &session);
        let rejection = CaptchaProtected::<Json<LoginInput>>::from_request(&request)
            .err()
            .expect("second use rejected");
        let response = rejection.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json: Value = serde_json::from_slice(response.body()).expect("json body");
        assert_eq!(json["errors"]["captcha"][0]["rule"], "captcha");
    }

    #[tokio::test]
    async fn captcha_protected_extractor_rejects_missing_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let request = Request::from_parts(
            Method::POST,
            "/login".parse().expect("uri"),
            headers,
            Bytes::from_static(br#"{"captcha":"abcde"}"#),
        );
        let rejection = CaptchaProtected::<Json<LoginInput>>::from_request(&request)
            .err()
            .expect("missing session rejected");
        assert_eq!(
            rejection.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn stored_flow_never_persists_plaintext_and_verifies_once() {
        let store = MemoryCaptchaStore::new();
        let captcha = Captcha::default();

        let issued = captcha.issue_stored(&store).await.expect("issue");
        assert_eq!(issued.id.len(), CHALLENGE_ID_LENGTH);
        assert!(issued.id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(issued.svg.starts_with("<svg "));
        assert_eq!(issued.expires_in, DEFAULT_TTL);
        assert_eq!(store.len(), 1);

        // Recover the answer from the SVG the way a solver would, to drive the
        // verification path without reaching into the store's plaintext (there
        // is none: only the hash is persisted).
        let answer = answer_from_svg(&issued.svg);
        assert_eq!(answer.len(), 5);

        assert!(
            captcha
                .verify_stored(&store, &issued.id, &answer.to_uppercase())
                .await
                .expect("verify"),
            "verification is case-insensitive"
        );
        assert!(store.is_empty(), "a verified challenge is consumed");
        assert!(
            !captcha
                .verify_stored(&store, &issued.id, &answer)
                .await
                .expect("verify"),
            "a challenge must be usable exactly once"
        );
    }

    #[tokio::test]
    async fn stored_flow_consumes_the_challenge_on_a_wrong_answer() {
        let store = MemoryCaptchaStore::new();
        let captcha = Captcha::default();
        let issued = captcha.issue_stored(&store).await.expect("issue");
        let answer = answer_from_svg(&issued.svg);

        assert!(
            !captcha
                .verify_stored(&store, &issued.id, "definitely-wrong")
                .await
                .expect("verify")
        );
        assert!(store.is_empty(), "a failed attempt also consumes it");
        assert!(
            !captcha
                .verify_stored(&store, &issued.id, &answer)
                .await
                .expect("verify"),
            "the right answer cannot rescue a consumed challenge"
        );
    }

    #[tokio::test]
    async fn stored_flow_rejects_unknown_empty_and_oversized_ids() {
        let store = MemoryCaptchaStore::new();
        let captcha = Captcha::default();
        let issued = captcha.issue_stored(&store).await.expect("issue");
        let answer = answer_from_svg(&issued.svg);

        for id in ["", "   ", "0".repeat(200).as_str(), "deadbeef"] {
            assert!(
                !captcha
                    .verify_stored(&store, id, &answer)
                    .await
                    .expect("verify"),
                "id `{id}` must not verify"
            );
        }
        assert_eq!(store.len(), 1, "no rejected id may consume the challenge");
    }

    #[tokio::test]
    async fn stored_challenges_expire() {
        let captcha = Captcha::new(CaptchaConfig {
            ttl: Duration::from_secs(1),
            ..CaptchaConfig::default()
        })
        .expect("valid config");
        let store = MemoryCaptchaStore::new();
        let issued = captcha.issue_stored(&store).await.expect("issue");
        let answer = answer_from_svg(&issued.svg);

        // Rewrite the row as already expired rather than sleeping.
        let expired = StoredChallenge {
            id: issued.id.clone(),
            answer_hash: hash_answer(&answer),
            expires_at: SystemTime::now() - Duration::from_secs(1),
        };
        store.take(&issued.id).await.expect("take");
        store.insert(expired).await.expect("insert");

        assert!(
            !captcha
                .verify_stored(&store, &issued.id, &answer)
                .await
                .expect("verify")
        );
        assert!(store.is_empty(), "an expired challenge is still claimed");
    }

    #[tokio::test]
    async fn challenge_route_is_installed_only_with_a_store() {
        let install = |feature: CaptchaFeature| {
            phoenix_plugin::FeatureSet::new()
                .plugin(feature)
                .expect("feature installs")
        };
        assert!(
            install(CaptchaFeature::new()).into_migrations().is_empty(),
            "the session flow needs no table"
        );
        let router = install(CaptchaFeature::new())
            .into_routes()
            .build()
            .expect("router builds");
        assert!(router.url("captcha.challenge", &[]).is_err());

        let store = Arc::new(MemoryCaptchaStore::new());
        let stored =
            || CaptchaFeature::new().with_store(Arc::clone(&store) as Arc<dyn CaptchaStore>);
        assert_eq!(
            install(stored())
                .into_migrations()
                .iter()
                .map(Migration::id)
                .collect::<Vec<_>>(),
            vec![CAPTCHAS_MIGRATION_ID]
        );
        let router = install(stored())
            .into_routes()
            .build()
            .expect("router builds");
        assert_eq!(
            router.url("captcha.challenge", &[]).expect("named"),
            "/captcha/challenge"
        );

        let response = router
            .handle(Request::new(
                Method::GET,
                "/captcha/challenge".parse().expect("uri"),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store, no-cache, must-revalidate"
        );
        let body: Value = serde_json::from_slice(response.body()).expect("json body");
        let id = body["id"].as_str().expect("id");
        assert_eq!(id.len(), CHALLENGE_ID_LENGTH);
        assert!(body["svg"].as_str().expect("svg").starts_with("<svg "));
        assert_eq!(body["expires_in"], DEFAULT_TTL.as_secs());
        assert_eq!(store.len(), 1, "the route persisted the challenge");
    }

    #[test]
    fn captcha_error_response_matches_the_extractor_body() {
        let response = captcha_error_response("code");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json: Value = serde_json::from_slice(response.body()).expect("json body");
        assert_eq!(json["errors"]["code"][0]["rule"], "captcha");
    }

    /// Read the answer back out of a rendered SVG: each glyph is its own
    /// `<text …>G</text>`, in order.
    fn answer_from_svg(svg: &str) -> String {
        svg.split("</text>")
            .filter_map(|chunk| chunk.rsplit_once('>'))
            .map(|(_, glyph)| glyph.to_owned())
            .collect()
    }

    #[test]
    fn captcha_format_rule_checks_shape_only() {
        let rule = captcha_format(5);
        let check = |value: Value| {
            let data = json!({ "captcha": value });
            rule.validate(RuleContext {
                field: "captcha",
                value: data.get("captcha"),
                data: &data,
            })
        };
        assert!(check(Value::String("ab3De".to_owned())).is_ok());
        assert!(check(Value::String(" ab3De ".to_owned())).is_ok());
        assert!(check(Value::String("abc".to_owned())).is_err());
        assert!(check(Value::String("ab3D<".to_owned())).is_err());
        assert!(check(Value::Null).is_err());
        assert_eq!(rule.name(), "captcha");
    }
}
