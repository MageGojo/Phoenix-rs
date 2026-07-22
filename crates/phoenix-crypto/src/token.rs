use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use phoenix_http::{BoxFuture, IntoResponse, Middleware, Next, Request, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::jwt::{Jwt, JwtClaims, JwtError, JwtManager, bearer_token, current_timestamp};

const REFRESH_TOKEN_BYTES: usize = 32;
const MAX_REFRESH_TOKEN_LENGTH: usize = 512;

/// Only hashes of opaque refresh tokens are persisted.
#[derive(Clone, Deserialize, Serialize)]
pub struct RefreshRecord {
    pub token_hash: String,
    pub family_id: String,
    pub subject: String,
    pub custom: Value,
    pub expires_at: u64,
    pub used_at: Option<u64>,
    pub revoked: bool,
}

impl std::fmt::Debug for RefreshRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RefreshRecord")
            .field("token_hash", &self.token_hash)
            .field("family_id", &self.family_id)
            .field("subject", &self.subject)
            .field("expires_at", &self.expires_at)
            .field("used_at", &self.used_at)
            .field("revoked", &self.revoked)
            .field("custom", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum RotateRefresh {
    Rotated(RefreshRecord),
    Reused,
    Invalid,
}

/// Atomic persistence contract required by refresh rotation and access revocation.
pub trait TokenStore: Send + Sync + 'static {
    fn insert_refresh(&self, record: RefreshRecord) -> BoxFuture<Result<(), TokenStoreError>>;
    fn find_refresh(
        &self,
        token_hash: String,
        now: u64,
    ) -> BoxFuture<Result<Option<RefreshRecord>, TokenStoreError>>;
    fn rotate_refresh(
        &self,
        token_hash: String,
        replacement_hash: String,
        replacement_expires_at: u64,
        now: u64,
    ) -> BoxFuture<Result<RotateRefresh, TokenStoreError>>;
    fn revoke_family(
        &self,
        family_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>>;
    fn is_family_revoked(
        &self,
        family_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>>;
    fn revoke_access(
        &self,
        token_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>>;
    fn is_access_revoked(
        &self,
        token_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>>;
    fn purge_expired(&self, now: u64) -> BoxFuture<Result<(), TokenStoreError>>;
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TokenState {
    refresh: HashMap<String, RefreshRecord>,
    revoked_families: HashMap<String, u64>,
    revoked_access: HashMap<String, u64>,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryTokenStore {
    state: Arc<Mutex<TokenState>>,
}

impl MemoryTokenStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryTokenStore {
    fn insert_refresh(&self, record: RefreshRecord) -> BoxFuture<Result<(), TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let mut state = lock_state(&state);
            insert_refresh(&mut state, record)
        })
    }

    fn find_refresh(
        &self,
        token_hash: String,
        now: u64,
    ) -> BoxFuture<Result<Option<RefreshRecord>, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move { Ok(find_refresh(&lock_state(&state), &token_hash, now)) })
    }

    fn rotate_refresh(
        &self,
        token_hash: String,
        replacement_hash: String,
        replacement_expires_at: u64,
        now: u64,
    ) -> BoxFuture<Result<RotateRefresh, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            Ok(rotate_refresh(
                &mut lock_state(&state),
                &token_hash,
                replacement_hash,
                replacement_expires_at,
                now,
            ))
        })
    }

    fn revoke_family(
        &self,
        family_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            revoke_family(&mut lock_state(&state), &family_id, expires_at);
            Ok(())
        })
    }

    fn is_family_revoked(
        &self,
        family_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            Ok(lock_state(&state)
                .revoked_families
                .get(&family_id)
                .is_some_and(|expires_at| *expires_at > now))
        })
    }

    fn revoke_access(
        &self,
        token_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            lock_state(&state)
                .revoked_access
                .insert(token_id, expires_at);
            Ok(())
        })
    }

    fn is_access_revoked(
        &self,
        token_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            Ok(lock_state(&state)
                .revoked_access
                .get(&token_id)
                .is_some_and(|expires_at| *expires_at > now))
        })
    }

    fn purge_expired(&self, now: u64) -> BoxFuture<Result<(), TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            purge_expired(&mut lock_state(&state), now);
            Ok(())
        })
    }
}

/// Single-process durable JSON store using atomic same-directory replacement.
#[derive(Clone, Debug)]
pub struct FileTokenStore {
    path: Arc<PathBuf>,
    state: Arc<Mutex<TokenState>>,
}

impl FileTokenStore {
    /// Open an existing store or initialize empty state when the path is absent.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable or malformed persistent state.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TokenStoreError> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            TokenState::default()
        };
        Ok(Self {
            path: Arc::new(path),
            state: Arc::new(Mutex::new(state)),
        })
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut TokenState) -> Result<T, TokenStoreError>,
    ) -> Result<T, TokenStoreError> {
        let mut state = lock_state(&self.state);
        let mut candidate = state.clone();
        let output = operation(&mut candidate)?;
        persist_state(&self.path, &candidate)?;
        *state = candidate;
        Ok(output)
    }
}

impl TokenStore for FileTokenStore {
    fn insert_refresh(&self, record: RefreshRecord) -> BoxFuture<Result<(), TokenStoreError>> {
        let store = self.clone();
        Box::pin(async move { store.mutate(|state| insert_refresh(state, record)) })
    }

    fn find_refresh(
        &self,
        token_hash: String,
        now: u64,
    ) -> BoxFuture<Result<Option<RefreshRecord>, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move { Ok(find_refresh(&lock_state(&state), &token_hash, now)) })
    }

    fn rotate_refresh(
        &self,
        token_hash: String,
        replacement_hash: String,
        replacement_expires_at: u64,
        now: u64,
    ) -> BoxFuture<Result<RotateRefresh, TokenStoreError>> {
        let store = self.clone();
        Box::pin(async move {
            store.mutate(|state| {
                Ok(rotate_refresh(
                    state,
                    &token_hash,
                    replacement_hash,
                    replacement_expires_at,
                    now,
                ))
            })
        })
    }

    fn revoke_family(
        &self,
        family_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let store = self.clone();
        Box::pin(async move {
            store.mutate(|state| {
                revoke_family(state, &family_id, expires_at);
                Ok(())
            })
        })
    }

    fn is_family_revoked(
        &self,
        family_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            Ok(lock_state(&state)
                .revoked_families
                .get(&family_id)
                .is_some_and(|expires_at| *expires_at > now))
        })
    }

    fn revoke_access(
        &self,
        token_id: String,
        expires_at: u64,
    ) -> BoxFuture<Result<(), TokenStoreError>> {
        let store = self.clone();
        Box::pin(async move {
            store.mutate(|state| {
                state.revoked_access.insert(token_id, expires_at);
                Ok(())
            })
        })
    }

    fn is_access_revoked(
        &self,
        token_id: String,
        now: u64,
    ) -> BoxFuture<Result<bool, TokenStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            Ok(lock_state(&state)
                .revoked_access
                .get(&token_id)
                .is_some_and(|expires_at| *expires_at > now))
        })
    }

    fn purge_expired(&self, now: u64) -> BoxFuture<Result<(), TokenStoreError>> {
        let store = self.clone();
        Box::pin(async move {
            store.mutate(|state| {
                purge_expired(state, now);
                Ok(())
            })
        })
    }
}

fn lock_state(state: &Mutex<TokenState>) -> std::sync::MutexGuard<'_, TokenState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn insert_refresh(state: &mut TokenState, record: RefreshRecord) -> Result<(), TokenStoreError> {
    if state.refresh.contains_key(&record.token_hash) {
        return Err(TokenStoreError::DuplicateToken);
    }
    state.refresh.insert(record.token_hash.clone(), record);
    Ok(())
}

fn find_refresh(state: &TokenState, token_hash: &str, now: u64) -> Option<RefreshRecord> {
    let record = state.refresh.get(token_hash)?;
    (record.expires_at > now && !record.revoked).then(|| record.clone())
}

fn rotate_refresh(
    state: &mut TokenState,
    token_hash: &str,
    replacement_hash: String,
    replacement_expires_at: u64,
    now: u64,
) -> RotateRefresh {
    let Some(existing) = state.refresh.get(token_hash).cloned() else {
        return RotateRefresh::Invalid;
    };
    if existing.expires_at <= now
        || existing.revoked
        || state
            .revoked_families
            .get(&existing.family_id)
            .is_some_and(|expires_at| *expires_at > now)
    {
        return RotateRefresh::Invalid;
    }
    if existing.used_at.is_some() {
        let family_expires_at = state
            .refresh
            .values()
            .filter(|record| record.family_id == existing.family_id)
            .map(|record| record.expires_at)
            .max()
            .unwrap_or(existing.expires_at);
        revoke_family(state, &existing.family_id, family_expires_at);
        return RotateRefresh::Reused;
    }
    if let Some(record) = state.refresh.get_mut(token_hash) {
        record.used_at = Some(now);
    }
    let replacement = RefreshRecord {
        token_hash: replacement_hash,
        family_id: existing.family_id,
        subject: existing.subject,
        custom: existing.custom,
        expires_at: replacement_expires_at,
        used_at: None,
        revoked: false,
    };
    state
        .refresh
        .insert(replacement.token_hash.clone(), replacement.clone());
    RotateRefresh::Rotated(replacement)
}

fn revoke_family(state: &mut TokenState, family_id: &str, expires_at: u64) {
    state
        .revoked_families
        .entry(family_id.to_owned())
        .and_modify(|existing| *existing = (*existing).max(expires_at))
        .or_insert(expires_at);
    for record in state.refresh.values_mut() {
        if record.family_id == family_id {
            record.revoked = true;
        }
    }
}

fn purge_expired(state: &mut TokenState, now: u64) {
    state.refresh.retain(|_, record| record.expires_at > now);
    state
        .revoked_families
        .retain(|_, expires_at| *expires_at > now);
    state
        .revoked_access
        .retain(|_, expires_at| *expires_at > now);
}

fn persist_state(path: &Path, state: &TokenState) -> Result<(), TokenStoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(TokenStoreError::InvalidPath)?;
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}",
        URL_SAFE_NO_PAD.encode(rand::random::<[u8; 12]>())
    ));
    let bytes = serde_json::to_vec(state)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(TokenStoreError::Io)
}

/// Access/refresh credentials. Debug output never includes either token.
#[derive(Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
}

impl std::fmt::Debug for TokenPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokenPair")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish()
    }
}

/// Issues rotating refresh families and checks stateful access revocation.
pub struct TokenService<Store> {
    jwt: JwtManager,
    store: Arc<Store>,
    refresh_ttl: Duration,
}

impl<Store> Clone for TokenService<Store> {
    fn clone(&self) -> Self {
        Self {
            jwt: self.jwt.clone(),
            store: Arc::clone(&self.store),
            refresh_ttl: self.refresh_ttl,
        }
    }
}

impl<Store: TokenStore> TokenService<Store> {
    /// # Errors
    ///
    /// Returns an error when refresh TTL is zero.
    pub fn new(
        jwt: JwtManager,
        store: Arc<Store>,
        refresh_ttl: Duration,
    ) -> Result<Self, TokenError> {
        if refresh_ttl.is_zero() {
            return Err(TokenError::InvalidRefreshTtl);
        }
        if refresh_ttl < jwt.access_token_ttl() {
            return Err(TokenError::RefreshTtlShorterThanAccess);
        }
        Ok(Self {
            jwt,
            store,
            refresh_ttl,
        })
    }

    /// Issue a new access token and refresh family.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid claims, clock failure, or persistence failure.
    pub async fn issue<T: Serialize>(
        &self,
        subject: impl Into<String>,
        custom: T,
    ) -> Result<TokenPair, TokenError> {
        let now = current_timestamp()?;
        let subject = subject.into();
        let custom = serde_json::to_value(custom)?;
        let family_id = random_token(16);
        let refresh_token = random_token(REFRESH_TOKEN_BYTES);
        let refresh_expires_at = now.saturating_add(self.refresh_ttl.as_secs());
        let (access_token, claims) = self.jwt.issue_at_in_family(
            subject.clone(),
            custom.clone(),
            now,
            Some(family_id.clone()),
        )?;
        self.store
            .insert_refresh(RefreshRecord {
                token_hash: hash_refresh(&refresh_token),
                family_id,
                subject,
                custom,
                expires_at: refresh_expires_at,
                used_at: None,
                revoked: false,
            })
            .await?;
        Ok(TokenPair {
            access_token,
            refresh_token,
            access_expires_at: claims.exp,
            refresh_expires_at,
        })
    }

    /// Rotate a refresh token exactly once. Reuse revokes the whole family.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed/expired/reused tokens or persistence failure.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, TokenError> {
        validate_refresh(refresh_token)?;
        let now = current_timestamp()?;
        let presented_hash = hash_refresh(refresh_token);
        let snapshot = self
            .store
            .find_refresh(presented_hash.clone(), now)
            .await?
            .ok_or(TokenError::InvalidRefresh)?;
        let replacement = random_token(REFRESH_TOKEN_BYTES);
        let replacement_hash = hash_refresh(&replacement);
        let refresh_expires_at = now.saturating_add(self.refresh_ttl.as_secs());
        let (access_token, claims) = self.jwt.issue_at_in_family(
            snapshot.subject.clone(),
            snapshot.custom.clone(),
            now,
            Some(snapshot.family_id.clone()),
        )?;
        match self
            .store
            .rotate_refresh(presented_hash, replacement_hash, refresh_expires_at, now)
            .await?
        {
            RotateRefresh::Rotated(_) => Ok(TokenPair {
                access_token,
                refresh_token: replacement,
                access_expires_at: claims.exp,
                refresh_expires_at,
            }),
            RotateRefresh::Reused => Err(TokenError::RefreshReuse),
            RotateRefresh::Invalid => Err(TokenError::InvalidRefresh),
        }
    }

    /// Verify cryptographic claims plus access-token and family revocation state.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or revoked access tokens.
    pub async fn verify_access<T: DeserializeOwned>(
        &self,
        token: &str,
    ) -> Result<JwtClaims<T>, TokenError> {
        let claims = self.jwt.verify::<T>(token)?;
        let now = current_timestamp()?;
        if self
            .store
            .is_access_revoked(claims.jti.clone(), now)
            .await?
        {
            return Err(TokenError::AccessRevoked);
        }
        if let Some(family_id) = &claims.sid
            && self.store.is_family_revoked(family_id.clone(), now).await?
        {
            return Err(TokenError::FamilyRevoked);
        }
        Ok(claims)
    }

    /// Revoke one access token until its expiration.
    ///
    /// # Errors
    ///
    /// Returns an error when the access token is invalid or persistence fails.
    pub async fn revoke_access(&self, token: &str) -> Result<(), TokenError> {
        let claims = self.jwt.verify::<Value>(token)?;
        self.store
            .revoke_access(claims.jti, claims.exp)
            .await
            .map_err(Into::into)
    }

    /// Revoke a complete refresh family and every family-bound access token.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock is invalid or persistence fails.
    pub async fn revoke_family(&self, family_id: impl Into<String>) -> Result<(), TokenError> {
        let expires_at = current_timestamp()?.saturating_add(self.refresh_ttl.as_secs());
        self.store
            .revoke_family(family_id.into(), expires_at)
            .await
            .map_err(Into::into)
    }

    /// Remove expired refresh and revocation records.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock is invalid or persistence fails.
    pub async fn purge_expired(&self) -> Result<(), TokenError> {
        self.store
            .purge_expired(current_timestamp()?)
            .await
            .map_err(Into::into)
    }
}

/// Stateful Bearer authentication that rejects revoked access/family IDs.
pub struct StatefulJwtAuth<T, Store> {
    service: Arc<TokenService<Store>>,
    marker: PhantomData<fn() -> T>,
}

impl<T, Store> StatefulJwtAuth<T, Store> {
    #[must_use]
    pub fn new(service: Arc<TokenService<Store>>) -> Self {
        Self {
            service,
            marker: PhantomData,
        }
    }
}

impl<T, Store> Middleware for StatefulJwtAuth<T, Store>
where
    T: DeserializeOwned + Send + Sync + 'static,
    Store: TokenStore,
{
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            let token = request
                .headers()
                .get(phoenix_http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(bearer_token);
            let Some(token) = token else {
                return TokenError::InvalidAccess.into_response();
            };
            let Ok(claims) = service.verify_access::<T>(token).await else {
                return TokenError::InvalidAccess.into_response();
            };
            request.extensions_mut().insert(Jwt::from_verified(claims));
            next.run(request).await
        })
    }
}

fn random_token(bytes: usize) -> String {
    let random = (0..bytes).map(|_| rand::random::<u8>()).collect::<Vec<_>>();
    URL_SAFE_NO_PAD.encode(random)
}

fn hash_refresh(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn validate_refresh(token: &str) -> Result<(), TokenError> {
    if token.len() > MAX_REFRESH_TOKEN_LENGTH {
        return Err(TokenError::InvalidRefresh);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| TokenError::InvalidRefresh)?;
    if decoded.len() != REFRESH_TOKEN_BYTES {
        return Err(TokenError::InvalidRefresh);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("token-store I/O failed")]
    Io(#[from] std::io::Error),
    #[error("token-store serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("token-store path is invalid")]
    InvalidPath,
    #[error("token hash collision")]
    DuplicateToken,
}

#[derive(Debug, Error)]
pub enum TokenError {
    #[error(transparent)]
    Jwt(#[from] JwtError),
    #[error(transparent)]
    Store(#[from] TokenStoreError),
    #[error("token custom claims are invalid")]
    Claims(#[from] serde_json::Error),
    #[error("refresh-token TTL must be greater than zero")]
    InvalidRefreshTtl,
    #[error("refresh-token TTL cannot be shorter than the access-token TTL")]
    RefreshTtlShorterThanAccess,
    #[error("refresh token is invalid or expired")]
    InvalidRefresh,
    #[error("refresh token reuse detected; token family revoked")]
    RefreshReuse,
    #[error("access token is invalid")]
    InvalidAccess,
    #[error("access token was revoked")]
    AccessRevoked,
    #[error("access token family was revoked")]
    FamilyRevoked,
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        let mut response =
            Response::text("Unauthorized").with_status(phoenix_http::StatusCode::UNAUTHORIZED);
        response.headers_mut().insert(
            phoenix_http::header::WWW_AUTHENTICATE,
            phoenix_http::HeaderValue::from_static("Bearer"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use phoenix_http::{Method, StatusCode, typed};
    use phoenix_routing::Routes;
    use serde_json::json;

    use super::*;
    use crate::{JwtConfig, JwtKey};

    fn service<Store: TokenStore>(store: Arc<Store>) -> TokenService<Store> {
        TokenService::new(
            JwtManager::new(
                JwtKey::new("active", [8_u8; 32]).unwrap(),
                JwtConfig::new(Duration::from_mins(5)),
            )
            .unwrap(),
            store,
            Duration::from_hours(720),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn rotates_once_detects_reuse_and_revokes_the_family() {
        let service = service(Arc::new(MemoryTokenStore::new()));
        let initial = service
            .issue("user-1", json!({ "role": "member" }))
            .await
            .unwrap();
        assert!(format!("{initial:?}").contains("[REDACTED]"));
        let rotated = service.refresh(&initial.refresh_token).await.unwrap();
        assert_ne!(initial.refresh_token, rotated.refresh_token);
        assert!(matches!(
            service.refresh(&initial.refresh_token).await,
            Err(TokenError::RefreshReuse)
        ));
        assert!(matches!(
            service.verify_access::<Value>(&rotated.access_token).await,
            Err(TokenError::FamilyRevoked)
        ));
    }

    #[tokio::test]
    async fn revokes_individual_access_tokens_and_stateful_middleware_enforces_it() {
        let service = Arc::new(service(Arc::new(MemoryTokenStore::new())));
        let pair = service
            .issue("user-2", json!({ "role": "admin" }))
            .await
            .unwrap();
        let router = Routes::new()
            .get(
                "/private",
                typed(|claims: Jwt<Value>| async move { claims.sub.clone() }),
            )
            .with_middleware(StatefulJwtAuth::<Value, _>::new(Arc::clone(&service)))
            .build()
            .unwrap();
        let mut request = Request::new(Method::GET, "/private".parse().unwrap());
        request.headers_mut().insert(
            phoenix_http::header::AUTHORIZATION,
            phoenix_http::HeaderValue::from_str(&format!("bEaReR {}", pair.access_token)).unwrap(),
        );
        assert_eq!(router.handle(request).await.status(), StatusCode::OK);

        service.revoke_access(&pair.access_token).await.unwrap();
        assert!(matches!(
            service.verify_access::<Value>(&pair.access_token).await,
            Err(TokenError::AccessRevoked)
        ));
    }

    #[tokio::test]
    async fn file_store_survives_reopen_without_persisting_plaintext_refresh_tokens() {
        let path = std::env::temp_dir().join(format!(
            "phoenix-token-store-{}.json",
            URL_SAFE_NO_PAD.encode(rand::random::<[u8; 12]>())
        ));
        let first = service(Arc::new(FileTokenStore::open(&path).unwrap()));
        let pair = first
            .issue("user-3", json!({ "role": "member" }))
            .await
            .unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains(&pair.refresh_token));
        drop(first);

        let reopened = service(Arc::new(FileTokenStore::open(&path).unwrap()));
        let rotated = reopened.refresh(&pair.refresh_token).await.unwrap();
        assert!(
            reopened
                .verify_access::<Value>(&rotated.access_token)
                .await
                .is_ok()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn concurrent_refresh_allows_one_rotation_and_revokes_on_reuse() {
        let service = Arc::new(service(Arc::new(MemoryTokenStore::new())));
        let initial = service.issue("user-4", json!({})).await.unwrap();
        let first_service = Arc::clone(&service);
        let first_token = initial.refresh_token.clone();
        let first = tokio::spawn(async move { first_service.refresh(&first_token).await });
        let second_service = Arc::clone(&service);
        let second_token = initial.refresh_token;
        let second = tokio::spawn(async move { second_service.refresh(&second_token).await });
        let outcomes = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|result| matches!(result, Err(TokenError::RefreshReuse)))
                .count(),
            1
        );
        let rotated = outcomes.into_iter().find_map(Result::ok).unwrap();
        assert!(matches!(
            service.verify_access::<Value>(&rotated.access_token).await,
            Err(TokenError::FamilyRevoked)
        ));
    }

    #[tokio::test]
    async fn file_store_persists_access_and_family_revocations() {
        let path = std::env::temp_dir().join(format!(
            "phoenix-token-revocations-{}.json",
            URL_SAFE_NO_PAD.encode(rand::random::<[u8; 12]>())
        ));
        let first = service(Arc::new(FileTokenStore::open(&path).unwrap()));
        let access_pair = first.issue("user-5", json!({})).await.unwrap();
        first
            .revoke_access(&access_pair.access_token)
            .await
            .unwrap();
        let family_pair = first.issue("user-6", json!({})).await.unwrap();
        let family_id = first
            .verify_access::<Value>(&family_pair.access_token)
            .await
            .unwrap()
            .sid
            .unwrap();
        first.revoke_family(family_id).await.unwrap();
        drop(first);

        let reopened = service(Arc::new(FileTokenStore::open(&path).unwrap()));
        assert!(matches!(
            reopened
                .verify_access::<Value>(&access_pair.access_token)
                .await,
            Err(TokenError::AccessRevoked)
        ));
        assert!(matches!(
            reopened
                .verify_access::<Value>(&family_pair.access_token)
                .await,
            Err(TokenError::FamilyRevoked)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn failed_file_persistence_rolls_back_memory_state() {
        let root = std::env::temp_dir().join(format!(
            "phoenix-token-rollback-{}",
            URL_SAFE_NO_PAD.encode(rand::random::<[u8; 12]>())
        ));
        let parent = root.join("missing");
        let path = parent.join("tokens.json");
        let store = FileTokenStore::open(&path).unwrap();
        let record = RefreshRecord {
            token_hash: "stable-hash".to_owned(),
            family_id: "family".to_owned(),
            subject: "subject".to_owned(),
            custom: json!({}),
            expires_at: u64::MAX,
            used_at: None,
            revoked: false,
        };
        assert!(matches!(
            store.insert_refresh(record.clone()).await,
            Err(TokenStoreError::Io(_))
        ));
        std::fs::create_dir_all(&parent).unwrap();
        store.insert_refresh(record).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_lifetime_must_cover_access_lifetime() {
        let jwt = JwtManager::new(
            JwtKey::new("active", [8_u8; 32]).unwrap(),
            JwtConfig::new(Duration::from_mins(5)),
        )
        .unwrap();
        assert!(matches!(
            TokenService::new(
                jwt,
                Arc::new(MemoryTokenStore::new()),
                Duration::from_mins(4),
            ),
            Err(TokenError::RefreshTtlShorterThanAccess)
        ));
    }

    #[test]
    fn reuse_tombstone_covers_the_newest_family_member() {
        let old = RefreshRecord {
            token_hash: "old".to_owned(),
            family_id: "family".to_owned(),
            subject: "subject".to_owned(),
            custom: json!({}),
            expires_at: 200,
            used_at: Some(100),
            revoked: false,
        };
        let replacement = RefreshRecord {
            token_hash: "replacement".to_owned(),
            expires_at: 900,
            used_at: None,
            ..old.clone()
        };
        let mut state = TokenState {
            refresh: HashMap::from([
                (old.token_hash.clone(), old),
                (replacement.token_hash.clone(), replacement),
            ]),
            ..TokenState::default()
        };
        assert!(matches!(
            rotate_refresh(&mut state, "old", "unused".to_owned(), 1_000, 150),
            RotateRefresh::Reused
        ));
        assert_eq!(state.revoked_families.get("family"), Some(&900));
    }
}
