use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use phoenix_http::BoxFuture;
use serde_json::Value;

/// Versioned server-side session state returned by a shared backend.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSnapshot {
    pub values: HashMap<String, Value>,
    pub version: u64,
    pub expires_at: u64,
}

/// Result of an atomic session mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionWrite {
    Saved { version: u64 },
    Conflict,
    Missing,
    Collision,
}

/// Sanitized backend error. The detail is available to operators, not HTTP clients.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionBackendError(pub String);

impl std::fmt::Display for SessionBackendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("session backend failed")
    }
}

impl std::error::Error for SessionBackendError {}

/// Distributed session contract.
///
/// Implementations must make each mutation atomic and enforce the supplied version.
pub trait SessionBackend: Send + Sync + 'static {
    fn load(
        &self,
        id: String,
        now: u64,
        refresh_expires_at: u64,
    ) -> BoxFuture<Result<Option<SessionSnapshot>, SessionBackendError>>;

    fn create(
        &self,
        id: String,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>>;

    fn save(
        &self,
        id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>>;

    fn rotate(
        &self,
        old_id: String,
        new_id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>>;

    fn delete(
        &self,
        id: String,
        expected_version: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>>;
}

/// Shared in-memory reference backend used by local applications and contract tests.
#[derive(Clone, Debug, Default)]
pub struct MemorySessionBackend {
    records: Arc<Mutex<HashMap<String, SessionSnapshot>>>,
}

impl MemorySessionBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionBackend for MemorySessionBackend {
    fn load(
        &self,
        id: String,
        now: u64,
        refresh_expires_at: u64,
    ) -> BoxFuture<Result<Option<SessionSnapshot>, SessionBackendError>> {
        let records = Arc::clone(&self.records);
        Box::pin(async move {
            let mut records = lock(&records);
            records.retain(|_, record| record.expires_at > now);
            Ok(records.get_mut(&id).map(|record| {
                record.expires_at = record.expires_at.max(refresh_expires_at);
                record.clone()
            }))
        })
    }

    fn create(
        &self,
        id: String,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let records = Arc::clone(&self.records);
        Box::pin(async move {
            let mut records = lock(&records);
            if records.contains_key(&id) {
                return Ok(SessionWrite::Collision);
            }
            records.insert(
                id,
                SessionSnapshot {
                    values,
                    version: 1,
                    expires_at,
                },
            );
            Ok(SessionWrite::Saved { version: 1 })
        })
    }

    fn save(
        &self,
        id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let records = Arc::clone(&self.records);
        Box::pin(async move {
            let mut records = lock(&records);
            let Some(record) = records.get_mut(&id) else {
                return Ok(SessionWrite::Missing);
            };
            if record.version != expected_version {
                return Ok(SessionWrite::Conflict);
            }
            record.version = record.version.saturating_add(1);
            record.values = values;
            record.expires_at = expires_at;
            Ok(SessionWrite::Saved {
                version: record.version,
            })
        })
    }

    fn rotate(
        &self,
        old_id: String,
        new_id: String,
        expected_version: u64,
        values: HashMap<String, Value>,
        expires_at: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let records = Arc::clone(&self.records);
        Box::pin(async move {
            let mut records = lock(&records);
            if records.contains_key(&new_id) {
                return Ok(SessionWrite::Collision);
            }
            let Some(old) = records.get(&old_id) else {
                return Ok(SessionWrite::Missing);
            };
            if old.version != expected_version {
                return Ok(SessionWrite::Conflict);
            }
            let version = old.version.saturating_add(1);
            records.remove(&old_id);
            records.insert(
                new_id,
                SessionSnapshot {
                    values,
                    version,
                    expires_at,
                },
            );
            Ok(SessionWrite::Saved { version })
        })
    }

    fn delete(
        &self,
        id: String,
        expected_version: u64,
    ) -> BoxFuture<Result<SessionWrite, SessionBackendError>> {
        let records = Arc::clone(&self.records);
        Box::pin(async move {
            let mut records = lock(&records);
            let Some(record) = records.get(&id) else {
                return Ok(SessionWrite::Missing);
            };
            if record.version != expected_version {
                return Ok(SessionWrite::Conflict);
            }
            records.remove(&id);
            Ok(SessionWrite::Saved {
                version: expected_version.saturating_add(1),
            })
        })
    }
}

fn lock(
    records: &Mutex<HashMap<String, SessionSnapshot>>,
) -> std::sync::MutexGuard<'_, HashMap<String, SessionSnapshot>> {
    records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn two_handles_observe_cas_conflicts_rotation_delete_and_ttl() {
        let first = MemorySessionBackend::new();
        let second = first.clone();
        assert_eq!(
            first
                .create(
                    "old".to_owned(),
                    HashMap::from([("n".to_owned(), json!(1))]),
                    200
                )
                .await
                .unwrap(),
            SessionWrite::Saved { version: 1 }
        );
        let left = first
            .load("old".to_owned(), 100, 250)
            .await
            .unwrap()
            .unwrap();
        let right = second
            .load("old".to_owned(), 100, 250)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first
                .save("old".to_owned(), left.version, HashMap::new(), 300)
                .await
                .unwrap(),
            SessionWrite::Saved { version: 2 }
        );
        assert_eq!(
            second
                .save("old".to_owned(), right.version, HashMap::new(), 300)
                .await
                .unwrap(),
            SessionWrite::Conflict
        );
        assert_eq!(
            second
                .rotate("old".to_owned(), "new".to_owned(), 2, HashMap::new(), 400)
                .await
                .unwrap(),
            SessionWrite::Saved { version: 3 }
        );
        assert!(
            first
                .load("old".to_owned(), 150, 400)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            first.delete("new".to_owned(), 3).await.unwrap(),
            SessionWrite::Saved { version: 4 }
        );
        first
            .create("expired".to_owned(), HashMap::new(), 10)
            .await
            .unwrap();
        assert!(
            second
                .load("expired".to_owned(), 11, 20)
                .await
                .unwrap()
                .is_none()
        );
    }
}
