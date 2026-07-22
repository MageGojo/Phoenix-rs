use std::{
    collections::HashMap,
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::{HeaderValue, header};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use phoenix_http::{
    BoxFuture, FromRequest, IntoResponse, Middleware, Next, Request, Response, StatusCode,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use zeroize::Zeroizing;

const MIN_SECRET_BYTES: usize = 32;

/// Standard JWT claims plus an application-defined payload.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JwtClaims<T> {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    pub nbf: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    #[serde(flatten)]
    pub custom: T,
}

/// A symmetric JWT key identified by `kid` for safe rotation.
#[derive(Clone)]
pub struct JwtKey {
    id: String,
    secret: Zeroizing<Vec<u8>>,
}

impl JwtKey {
    /// Construct an HS256 key. Secrets shorter than 256 bits are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty key ID or a secret shorter than 32 bytes.
    pub fn new(id: impl Into<String>, secret: impl AsRef<[u8]>) -> Result<Self, JwtError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(JwtError::InvalidKeyId);
        }
        if secret.as_ref().len() < MIN_SECRET_BYTES {
            return Err(JwtError::WeakKey);
        }
        Ok(Self {
            id,
            secret: Zeroizing::new(secret.as_ref().to_vec()),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for JwtKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JwtKey")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct JwtConfig {
    pub ttl: Duration,
    pub leeway: Duration,
    pub issuer: Option<String>,
    pub audience: Option<String>,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_mins(15),
            leeway: Duration::from_secs(30),
            issuer: None,
            audience: None,
        }
    }
}

impl JwtConfig {
    #[must_use]
    pub const fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            leeway: Duration::from_secs(30),
            issuer: None,
            audience: None,
        }
    }

    #[must_use]
    pub const fn leeway(mut self, leeway: Duration) -> Self {
        self.leeway = leeway;
        self
    }

    #[must_use]
    pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    #[must_use]
    pub fn audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }
}

/// HS256 JWT issuer and verifier with explicit key rotation.
#[derive(Clone, Debug)]
pub struct JwtManager {
    active_key_id: String,
    keys: HashMap<String, JwtKey>,
    config: JwtConfig,
}

impl JwtManager {
    /// Create a manager with one active signing and verification key.
    ///
    /// # Errors
    ///
    /// Returns an error when TTL is zero or configured issuer/audience is empty.
    pub fn new(active: JwtKey, config: JwtConfig) -> Result<Self, JwtError> {
        validate_config(&config)?;
        let active_key_id = active.id.clone();
        Ok(Self {
            active_key_id,
            keys: HashMap::from([(active.id.clone(), active)]),
            config,
        })
    }

    /// Add an old or future verification key without changing the signing key.
    #[must_use]
    pub fn with_verification_key(mut self, key: JwtKey) -> Self {
        self.keys.insert(key.id.clone(), key);
        self
    }

    /// Issue a signed JWT using the active key and configured TTL.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock or serializer fails.
    pub fn issue<T: Serialize>(
        &self,
        subject: impl Into<String>,
        custom: T,
    ) -> Result<String, JwtError> {
        self.issue_at(subject.into(), custom, unix_timestamp()?)
    }

    fn issue_at<T: Serialize>(
        &self,
        subject: String,
        custom: T,
        now: u64,
    ) -> Result<String, JwtError> {
        validate_custom_claims(&custom)?;
        let key = self
            .keys
            .get(&self.active_key_id)
            .ok_or(JwtError::UnknownKey)?;
        let claims = JwtClaims {
            sub: subject,
            exp: now.saturating_add(self.config.ttl.as_secs()),
            iat: now,
            nbf: now,
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            custom,
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(key.id.clone());
        encode(
            &header,
            &claims,
            &EncodingKey::from_secret(key.secret.as_ref()),
        )
        .map_err(JwtError::Token)
    }

    /// Verify signature, algorithm, key ID, time claims, issuer, and audience.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unknown-key, expired, or policy-invalid tokens.
    pub fn verify<T: DeserializeOwned>(&self, token: &str) -> Result<JwtClaims<T>, JwtError> {
        let header = decode_header(token).map_err(JwtError::Token)?;
        if header.alg != Algorithm::HS256 {
            return Err(JwtError::InvalidAlgorithm);
        }
        let key_id = header.kid.ok_or(JwtError::MissingKeyId)?;
        let key = self.keys.get(&key_id).ok_or(JwtError::UnknownKey)?;
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = self.config.leeway.as_secs();
        validation.validate_nbf = true;
        let mut required = vec!["exp", "nbf", "sub"];
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(&[issuer]);
            required.push("iss");
        }
        if let Some(audience) = &self.config.audience {
            validation.set_audience(&[audience]);
            required.push("aud");
        }
        validation.set_required_spec_claims(&required);
        decode::<JwtClaims<T>>(
            token,
            &DecodingKey::from_secret(key.secret.as_ref()),
            &validation,
        )
        .map(|data| data.claims)
        .map_err(JwtError::Token)
    }
}

fn validate_custom_claims(custom: &impl Serialize) -> Result<(), JwtError> {
    const RESERVED: &[&str] = &["sub", "exp", "iat", "nbf", "iss", "aud"];
    let value = serde_json::to_value(custom).map_err(JwtError::CustomClaims)?;
    let object = value
        .as_object()
        .ok_or(JwtError::CustomClaimsMustBeObject)?;
    if object.keys().any(|key| RESERVED.contains(&key.as_str())) {
        return Err(JwtError::ReservedClaim);
    }
    Ok(())
}

fn validate_config(config: &JwtConfig) -> Result<(), JwtError> {
    if config.ttl.is_zero() {
        return Err(JwtError::InvalidTtl);
    }
    if config
        .issuer
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
        || config
            .audience
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(JwtError::InvalidClaimPolicy);
    }
    Ok(())
}

/// Verified JWT claims extracted by [`JwtAuth`].
pub struct Jwt<T>(Arc<JwtClaims<T>>);

impl<T> Clone for Jwt<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for Jwt<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("Jwt").field(&"[VERIFIED]").finish()
    }
}

impl<T> Deref for Jwt<T> {
    type Target = JwtClaims<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> FromRequest for Jwt<T>
where
    T: Send + Sync + 'static,
{
    type Rejection = JwtRejection;

    fn from_request(request: &Request) -> Result<Self, Self::Rejection> {
        request
            .extensions()
            .get::<Self>()
            .cloned()
            .ok_or(JwtRejection)
    }
}

/// Bearer middleware that verifies a JWT and inserts typed claims into request extensions.
#[derive(Clone, Debug)]
pub struct JwtAuth<T> {
    manager: Arc<JwtManager>,
    marker: PhantomData<fn() -> T>,
}

impl<T> JwtAuth<T> {
    #[must_use]
    pub fn new(manager: impl Into<Arc<JwtManager>>) -> Self {
        Self {
            manager: manager.into(),
            marker: PhantomData,
        }
    }
}

impl<T> Middleware for JwtAuth<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            let token = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(bearer_token);
            let Some(token) = token else {
                return JwtRejection.into_response();
            };
            let Ok(claims) = manager.verify::<T>(token) else {
                return JwtRejection.into_response();
            };
            request.extensions_mut().insert(Jwt(Arc::new(claims)));
            next.run(request).await
        })
    }
}

fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && !token.chars().any(char::is_whitespace))
    .then_some(token)
}

#[derive(Clone, Copy, Debug, Error)]
#[error("A valid bearer token is required.")]
pub struct JwtRejection;

impl IntoResponse for JwtRejection {
    fn into_response(self) -> Response {
        let mut response = Response::text("Unauthorized").with_status(StatusCode::UNAUTHORIZED);
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        response
    }
}

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("JWT key IDs cannot be empty")]
    InvalidKeyId,
    #[error("JWT HS256 keys must contain at least 32 bytes")]
    WeakKey,
    #[error("JWT time-to-live must be greater than zero")]
    InvalidTtl,
    #[error("JWT issuer and audience policies cannot be empty")]
    InvalidClaimPolicy,
    #[error("JWT header is missing a key ID")]
    MissingKeyId,
    #[error("JWT key ID is not recognized")]
    UnknownKey,
    #[error("JWT algorithm is not allowed")]
    InvalidAlgorithm,
    #[error("JWT custom claims must serialize as an object")]
    CustomClaimsMustBeObject,
    #[error("JWT custom claims cannot redefine reserved claims")]
    ReservedClaim,
    #[error("JWT custom claim serialization failed")]
    CustomClaims(#[source] serde_json::Error),
    #[error("JWT processing failed")]
    Token(#[source] jsonwebtoken::errors::Error),
    #[error("the system clock is before the Unix epoch")]
    InvalidClock,
}

fn unix_timestamp() -> Result<u64, JwtError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| JwtError::InvalidClock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::{Method, typed};
    use phoenix_routing::Routes;

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct UserClaims {
        role: String,
    }

    #[derive(Serialize)]
    struct ConflictingClaims {
        sub: String,
    }

    fn manager(key_id: &str, byte: u8) -> JwtManager {
        JwtManager::new(
            JwtKey::new(key_id, [byte; 32]).unwrap(),
            JwtConfig::new(Duration::from_mins(5))
                .leeway(Duration::ZERO)
                .issuer("phoenix.test")
                .audience("api.test"),
        )
        .unwrap()
    }

    #[test]
    fn round_trips_and_rotates_keys_without_exposing_secrets() {
        let old = JwtKey::new("old", [7_u8; 32]).unwrap();
        assert!(format!("{old:?}").contains("[REDACTED]"));
        let old_manager = JwtManager::new(old.clone(), JwtConfig::default()).unwrap();
        let token = old_manager
            .issue(
                "user-1",
                UserClaims {
                    role: "admin".to_owned(),
                },
            )
            .unwrap();
        let rotated = JwtManager::new(
            JwtKey::new("current", [9_u8; 32]).unwrap(),
            JwtConfig::default(),
        )
        .unwrap()
        .with_verification_key(old);

        let claims = rotated.verify::<UserClaims>(&token).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.custom.role, "admin");
    }

    #[test]
    fn rejects_weak_unknown_and_expired_tokens() {
        assert!(matches!(
            JwtKey::new("weak", b"short"),
            Err(JwtError::WeakKey)
        ));
        let active_manager = manager("active", 3);
        let expired = active_manager
            .issue_at(
                "user-1".to_owned(),
                UserClaims {
                    role: "member".to_owned(),
                },
                1,
            )
            .unwrap();
        assert!(matches!(
            active_manager.verify::<UserClaims>(&expired),
            Err(JwtError::Token(_))
        ));

        let foreign = manager("foreign", 4)
            .issue(
                "user-1",
                UserClaims {
                    role: "member".to_owned(),
                },
            )
            .unwrap();
        assert!(matches!(
            active_manager.verify::<UserClaims>(&foreign),
            Err(JwtError::UnknownKey)
        ));
        assert!(matches!(
            active_manager.issue(
                "user-1",
                ConflictingClaims {
                    sub: "shadow".to_owned()
                }
            ),
            Err(JwtError::ReservedClaim)
        ));
    }

    #[tokio::test]
    async fn bearer_middleware_inserts_typed_verified_claims() {
        let manager = manager("active", 5);
        let token = manager
            .issue(
                "user-7",
                UserClaims {
                    role: "editor".to_owned(),
                },
            )
            .unwrap();
        let handler = typed(|claims: Jwt<UserClaims>| async move {
            format!("{}:{}", claims.sub, claims.custom.role)
        });
        let router = Routes::new()
            .get("/private", handler)
            .with_middleware(JwtAuth::<UserClaims>::new(Arc::new(manager)))
            .build()
            .unwrap();

        let missing = router
            .handle(Request::new(Method::GET, "/private".parse().unwrap()))
            .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(missing.headers()[header::WWW_AUTHENTICATE], "Bearer");

        let mut request = Request::new(Method::GET, "/private".parse().unwrap());
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let response = router.handle(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), "user-7:editor");
    }
}
