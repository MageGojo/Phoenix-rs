//! Toasty-backed [`CaptchaStore`] persisting the `captchas` table.
//!
//! [`MemoryCaptchaStore`](crate::MemoryCaptchaStore) keeps pending challenges in
//! process memory; [`DbCaptchaStore`] persists them through the Toasty ORM, so
//! one-time use holds across instances and survives a restart. Both implement
//! the same [`CaptchaStore`] trait.
//!
//! The [`CaptchaRow`] model mirrors the columns shipped by the `captchas`
//! migration. Register it in the application `models!(...)` so the shared
//! database knows the table:
//!
//! ```ignore
//! let db = Database::builder(models!(crate::*, phoenix_captcha::CaptchaRow))
//!     .connect(&url)
//!     .await?;
//! let feature = CaptchaFeature::new().with_store(Arc::new(DbCaptchaStore::new(db)));
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_database::Database;
use phoenix_http::BoxFuture;
use toasty::Model;

use crate::{CaptchaStore, CaptchaStoreError, StoredChallenge};

/// Table name declared by [`CaptchaRow`] and created by the `captchas`
/// migration, before any [`Database::table_prefix`].
pub const CAPTCHAS_TABLE: &str = "captchas";

/// One row of the `captchas` table, as a Toasty model.
///
/// Column mapping matches the `captchas` migration shipped by
/// [`CaptchaFeature`](crate::CaptchaFeature). `expires_at` stores a
/// [`SystemTime`] as a fixed-width nanosecond string so `TEXT` comparison stays
/// chronological (same encoding as `phoenix-notify`'s timestamps).
///
/// Applications register this model in their `models!(...)` set; queries stay
/// inside [`DbCaptchaStore`].
#[derive(Debug, Model)]
#[table = "captchas"]
pub struct CaptchaRow {
    /// Opaque challenge id handed to the client.
    #[key]
    pub id: String,
    /// SHA-256 hex digest of the trimmed, lowercased answer — never plaintext.
    pub answer_hash: String,
    /// Expiry timestamp (nanoseconds since the epoch, zero-padded to 20 chars).
    #[index]
    pub expires_at: String,
}

/// Toasty-backed [`CaptchaStore`].
///
/// Holds a cheaply cloneable [`Database`] handle (an `Arc` over the connection
/// pool); every call borrows a fresh handle, so the store is `Send + Sync` and
/// safe to share behind an `Arc<dyn CaptchaStore>`.
#[derive(Clone)]
pub struct DbCaptchaStore {
    database: Database,
}

impl DbCaptchaStore {
    /// Wrap a [`Database`] whose `models!(...)` set includes [`CaptchaRow`].
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

impl std::fmt::Debug for DbCaptchaStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DbCaptchaStore")
            .field("backend", &self.database.backend())
            .finish()
    }
}

impl CaptchaStore for DbCaptchaStore {
    fn insert(&self, challenge: StoredChallenge) -> BoxFuture<Result<(), CaptchaStoreError>> {
        let mut database = self.database.clone();
        Box::pin(async move {
            let existing = CaptchaRow::filter_by_id(challenge.id.clone())
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            if existing.is_some() {
                return Err(CaptchaStoreError::DuplicateId(challenge.id));
            }

            CaptchaRow::create()
                .id(challenge.id)
                .answer_hash(challenge.answer_hash)
                .expires_at(encode_time(challenge.expires_at))
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }

    fn take(&self, id: &str) -> BoxFuture<Result<Option<StoredChallenge>, CaptchaStoreError>> {
        let mut database = self.database.clone();
        let id = id.to_owned();
        Box::pin(async move {
            let Some(row) = CaptchaRow::filter_by_id(id.clone())
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?
            else {
                return Ok(None);
            };

            // Claim the row with the DELETE itself. Two concurrent takes can
            // both read the row above, but only one `DELETE` affects it — the
            // loser sees 0 rows and reports "no pending challenge". Reading and
            // then unconditionally trusting the read would let a double-submit
            // spend the same challenge twice.
            let table = database.table_name(CAPTCHAS_TABLE);
            let placeholder = database.backend().placeholder(1);
            let deleted =
                toasty::sql::statement(format!("DELETE FROM {table} WHERE id = {placeholder}"))
                    .bind(id)
                    .exec(database.toasty_mut())
                    .await
                    .map_err(|error| to_store_error(&error))?;
            if deleted == 0 {
                return Ok(None);
            }

            let challenge = row_into_challenge(row)?;
            Ok((!challenge.is_expired_at(SystemTime::now())).then_some(challenge))
        })
    }

    fn purge_expired(&self, now: SystemTime) -> BoxFuture<Result<u64, CaptchaStoreError>> {
        let mut database = self.database.clone();
        Box::pin(async move {
            let table = database.table_name(CAPTCHAS_TABLE);
            let placeholder = database.backend().placeholder(1);
            toasty::sql::statement(format!(
                "DELETE FROM {table} WHERE expires_at <= {placeholder}"
            ))
            .bind(encode_time(now))
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))
        })
    }
}

fn row_into_challenge(row: CaptchaRow) -> Result<StoredChallenge, CaptchaStoreError> {
    Ok(StoredChallenge {
        id: row.id,
        answer_hash: row.answer_hash,
        expires_at: decode_time(&row.expires_at)?,
    })
}

fn to_store_error(error: &toasty::Error) -> CaptchaStoreError {
    CaptchaStoreError::Backend(error.to_string())
}

/// Encode a [`SystemTime`] as a fixed-width, lexicographically sortable
/// nanoseconds-since-epoch string, so `expires_at <= ?` compares chronologically
/// even on a `TEXT` column.
fn encode_time(time: SystemTime) -> String {
    let nanos = time
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or_default();
    let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
    format!("{nanos:020}")
}

fn decode_time(text: &str) -> Result<SystemTime, CaptchaStoreError> {
    let nanos: u64 = text.trim().parse().map_err(|_| {
        CaptchaStoreError::Backend(format!("invalid stored captcha expiry `{text}`"))
    })?;
    Ok(UNIX_EPOCH + Duration::from_nanos(nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip_and_sort_lexicographically() {
        let early = UNIX_EPOCH + Duration::from_micros(1);
        let late = UNIX_EPOCH + Duration::from_hours(500_000);
        assert!(encode_time(early) < encode_time(late));
        assert_eq!(encode_time(early).len(), 20);
        assert_eq!(decode_time(&encode_time(late)), Ok(late));
        assert!(decode_time("not-a-number").is_err());
    }
}
