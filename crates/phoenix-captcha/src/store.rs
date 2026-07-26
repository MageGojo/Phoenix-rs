//! Persistence seam for **session-less** captcha challenges.
//!
//! The session flow ([`Captcha::issue`](crate::Captcha::issue) /
//! [`Captcha::verify`](crate::Captcha::verify)) keeps the hashed answer in the
//! server-side session, so it needs a session cookie and inherits the session
//! lifetime. A [`CaptchaStore`] instead keeps the hashed answer under an opaque
//! challenge id that the client echoes back on submit, which is what stateless
//! API clients (mobile apps, third-party integrations) need — and it gives
//! one-time use a single authority across instances rather than per-session
//! storage.
//!
//! Two implementations ship: [`MemoryCaptchaStore`] (tests, single process) and
//! [`DbCaptchaStore`](crate::DbCaptchaStore), the Toasty-backed store. Custom
//! stores implement this trait; the async style matches the rest of the
//! workspace ([`BoxFuture`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use phoenix_http::BoxFuture;
use thiserror::Error;

/// One pending challenge held by a [`CaptchaStore`].
///
/// Only the **hash** of the lowercased answer is stored — never the plaintext,
/// exactly like the session flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredChallenge {
    /// Opaque, unguessable challenge id handed to the client.
    pub id: String,
    /// SHA-256 hex digest of the trimmed, lowercased answer.
    pub answer_hash: String,
    /// Instant after which the challenge is no longer accepted.
    pub expires_at: SystemTime,
}

impl StoredChallenge {
    /// Whether this challenge is already expired at `now`.
    #[must_use]
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        self.expires_at <= now
    }
}

/// Backing-store failure.
///
/// Verification **fails closed** on any of these: a store that cannot answer
/// never yields a passing captcha.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CaptchaStoreError {
    /// The underlying database or backend rejected the operation.
    #[error("captcha store backend error: {0}")]
    Backend(String),
    /// A challenge with this id already exists (id collision — never expected
    /// with 128 bits of entropy, so it is surfaced rather than swallowed).
    #[error("duplicate captcha challenge id `{0}`")]
    DuplicateId(String),
}

/// Persistence for session-less captcha challenges.
pub trait CaptchaStore: Send + Sync {
    /// Persist a freshly generated challenge.
    ///
    /// # Errors
    ///
    /// [`CaptchaStoreError::DuplicateId`] when `challenge.id` already exists,
    /// [`CaptchaStoreError::Backend`] on a backend failure.
    fn insert(&self, challenge: StoredChallenge) -> BoxFuture<Result<(), CaptchaStoreError>>;

    /// Atomically claim and remove the challenge `id`.
    ///
    /// This is the one-time-use primitive: for a given id **at most one**
    /// concurrent caller may observe `Ok(Some(_))`, every other caller sees
    /// `Ok(None)`. Implementations must not read and delete in two racy steps
    /// (see [`DbCaptchaStore`](crate::DbCaptchaStore), which claims the row
    /// with a conditional `DELETE` and checks the affected-row count).
    ///
    /// Expired challenges are removed and reported as `Ok(None)`.
    ///
    /// # Errors
    ///
    /// [`CaptchaStoreError::Backend`] on a backend failure.
    fn take(&self, id: &str) -> BoxFuture<Result<Option<StoredChallenge>, CaptchaStoreError>>;

    /// Delete every challenge that expired at or before `now`, returning how
    /// many rows were removed.
    ///
    /// Challenges are also removed on use, so this only reclaims the ones that
    /// were issued and never submitted. Call it from a scheduled job (see
    /// `docs/SCHEDULE.md`).
    ///
    /// # Errors
    ///
    /// [`CaptchaStoreError::Backend`] on a backend failure.
    fn purge_expired(&self, now: SystemTime) -> BoxFuture<Result<u64, CaptchaStoreError>>;
}

/// Thread-safe in-process [`CaptchaStore`] for tests and single-node setups.
///
/// `take` removes under the same lock that reads, so one-time use holds within
/// the process. It does **not** survive a restart and is not shared across
/// instances — use [`DbCaptchaStore`](crate::DbCaptchaStore) for those.
#[derive(Clone, Default)]
pub struct MemoryCaptchaStore {
    challenges: Arc<Mutex<HashMap<String, StoredChallenge>>>,
}

impl MemoryCaptchaStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of pending challenges (expired ones included until purged).
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no challenge is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, StoredChallenge>> {
        self.challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for MemoryCaptchaStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryCaptchaStore")
            .field("pending", &self.len())
            .finish()
    }
}

impl CaptchaStore for MemoryCaptchaStore {
    fn insert(&self, challenge: StoredChallenge) -> BoxFuture<Result<(), CaptchaStoreError>> {
        let result = {
            let mut challenges = self.lock();
            if challenges.contains_key(&challenge.id) {
                Err(CaptchaStoreError::DuplicateId(challenge.id))
            } else {
                challenges.insert(challenge.id.clone(), challenge);
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn take(&self, id: &str) -> BoxFuture<Result<Option<StoredChallenge>, CaptchaStoreError>> {
        // Remove under the lock: two concurrent takes cannot both see the row.
        let claimed = self.lock().remove(id);
        let result = Ok(claimed.filter(|challenge| !challenge.is_expired_at(SystemTime::now())));
        Box::pin(async move { result })
    }

    fn purge_expired(&self, now: SystemTime) -> BoxFuture<Result<u64, CaptchaStoreError>> {
        let removed = {
            let mut challenges = self.lock();
            let before = challenges.len();
            challenges.retain(|_, challenge| !challenge.is_expired_at(now));
            before - challenges.len()
        };
        let result = Ok(removed as u64);
        Box::pin(async move { result })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn challenge(id: &str, expires_at: SystemTime) -> StoredChallenge {
        StoredChallenge {
            id: id.to_owned(),
            answer_hash: "0".repeat(64),
            expires_at,
        }
    }

    fn future() -> SystemTime {
        SystemTime::now() + Duration::from_mins(5)
    }

    fn past() -> SystemTime {
        SystemTime::now() - Duration::from_secs(1)
    }

    #[tokio::test]
    async fn take_consumes_exactly_once() {
        let store = MemoryCaptchaStore::new();
        store
            .insert(challenge("a", future()))
            .await
            .expect("insert");
        assert_eq!(store.len(), 1);

        let taken = store.take("a").await.expect("take");
        assert_eq!(taken.map(|challenge| challenge.id), Some("a".to_owned()));
        assert!(store.is_empty());
        assert!(store.take("a").await.expect("take").is_none());
    }

    #[tokio::test]
    async fn duplicate_ids_are_rejected() {
        let store = MemoryCaptchaStore::new();
        store
            .insert(challenge("a", future()))
            .await
            .expect("insert");
        assert_eq!(
            store.insert(challenge("a", future())).await,
            Err(CaptchaStoreError::DuplicateId("a".to_owned()))
        );
    }

    #[tokio::test]
    async fn expired_challenges_are_removed_and_reported_missing() {
        let store = MemoryCaptchaStore::new();
        store.insert(challenge("a", past())).await.expect("insert");
        assert!(store.take("a").await.expect("take").is_none());
        assert!(store.is_empty(), "an expired take still claims the row");
    }

    #[tokio::test]
    async fn purge_expired_keeps_live_challenges() {
        let store = MemoryCaptchaStore::new();
        store
            .insert(challenge("old", past()))
            .await
            .expect("insert");
        store
            .insert(challenge("new", future()))
            .await
            .expect("insert");
        assert_eq!(store.purge_expired(SystemTime::now()).await, Ok(1));
        assert_eq!(store.len(), 1);
        assert!(store.take("new").await.expect("take").is_some());
    }
}
