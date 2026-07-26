//! Toasty-backed [`NotificationStore`] persisting the `notifications` table.
//!
//! [`MemoryNotificationStore`](crate::MemoryNotificationStore) keeps records in
//! process memory; [`DbNotificationStore`] persists them through the Toasty ORM
//! so notifications survive a restart. Both implement the same
//! [`NotificationStore`] trait, so a [`Notifier`](crate::Notifier) is assembled
//! the same way with either.
//!
//! The [`NotificationRow`] model mirrors the columns shipped by the
//! `notifications` migration (`id` / `notifiable_id` / `type` / `data` /
//! `read_at` / `created_at`). Register it in the application `models!(...)` so
//! the shared database knows the table:
//!
//! ```ignore
//! let db = Database::builder(models!(crate::*, phoenix_notify::NotificationRow))
//!     .connect(&url)
//!     .await?;
//! let store = Arc::new(DbNotificationStore::new(db));
//! let notifier = Notifier::new().with_mailer(mailer).with_store(store);
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_database::Database;
use phoenix_http::BoxFuture;
use toasty::Model;

use crate::{DatabaseNotification, NotificationStore, NotifyError};

/// One row of the `notifications` table, as a Toasty model.
///
/// Column mapping matches the `notifications` migration shipped by
/// [`NotifyFeature`](crate::NotifyFeature): the Rust field `notification_type`
/// maps to the SQL column `type` (a reserved word, quoted by Toasty), and
/// `read_at` / `created_at` store [`SystemTime`] as fixed-width nanosecond
/// strings so they remain chronologically sortable as `TEXT`.
///
/// Applications register this model in their `models!(...)` set; queries stay
/// inside [`DbNotificationStore`].
#[derive(Debug, Model)]
#[table = "notifications"]
pub struct NotificationRow {
    /// UUID primary key assigned by the [`Notifier`](crate::Notifier).
    #[key]
    pub id: String,
    /// Receiving notifiable identity.
    #[index]
    pub notifiable_id: String,
    /// Stable notification type; stored in the `type` column.
    #[column("type")]
    pub notification_type: String,
    /// JSON payload serialized to text.
    pub data: String,
    /// `read_at` timestamp (nanoseconds since the epoch), `None` while unread.
    pub read_at: Option<String>,
    /// Creation timestamp (nanoseconds since the epoch).
    pub created_at: String,
}

/// Toasty-backed [`NotificationStore`].
///
/// Holds a cheaply cloneable [`Database`] handle (an `Arc` over the connection
/// pool); every call borrows a fresh handle, so the store is `Send + Sync` and
/// safe to share behind an `Arc<dyn NotificationStore>`.
#[derive(Clone)]
pub struct DbNotificationStore {
    database: Database,
}

impl DbNotificationStore {
    /// Wrap a [`Database`] whose `models!(...)` set includes
    /// [`NotificationRow`].
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

impl std::fmt::Debug for DbNotificationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DbNotificationStore")
            .field("backend", &self.database.backend())
            .finish()
    }
}

impl NotificationStore for DbNotificationStore {
    fn insert(&self, record: DatabaseNotification) -> BoxFuture<Result<(), NotifyError>> {
        let mut database = self.database.clone();
        Box::pin(async move {
            let existing = NotificationRow::filter_by_id(record.id.clone())
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            if existing.is_some() {
                return Err(NotifyError::DuplicateNotification { id: record.id });
            }

            let data = serde_json::to_string(&record.data).map_err(|error| {
                NotifyError::Store(format!("serialize notification data: {error}"))
            })?;

            let mut builder = NotificationRow::create()
                .id(record.id.clone())
                .notifiable_id(record.notifiable_id.clone())
                .notification_type(record.notification_type.clone())
                .data(data)
                .created_at(encode_time(record.created_at));
            if let Some(read_at) = record.read_at {
                builder = builder.read_at(Some(encode_time(read_at)));
            }
            builder
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }

    fn mark_read(
        &self,
        id: &str,
        read_at: SystemTime,
    ) -> BoxFuture<Result<DatabaseNotification, NotifyError>> {
        let mut database = self.database.clone();
        let id = id.to_owned();
        Box::pin(async move {
            let Some(mut row) = NotificationRow::filter_by_id(id.clone())
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?
            else {
                return Err(NotifyError::NotificationNotFound { id });
            };

            // Idempotent: only the first mark stamps `read_at`.
            if row.read_at.is_none() {
                row.update()
                    .read_at(Some(encode_time(read_at)))
                    .exec(database.toasty_mut())
                    .await
                    .map_err(|error| to_store_error(&error))?;
            }
            row_into_record(row)
        })
    }

    fn unread_for(
        &self,
        notifiable_id: &str,
    ) -> BoxFuture<Result<Vec<DatabaseNotification>, NotifyError>> {
        let mut database = self.database.clone();
        let notifiable_id = notifiable_id.to_owned();
        Box::pin(async move {
            let filter = NotificationRow::fields()
                .notifiable_id()
                .eq(notifiable_id)
                .and(NotificationRow::fields().read_at().is_none());
            let rows = NotificationRow::all()
                .filter(filter)
                .order_by(NotificationRow::fields().created_at().asc())
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            rows.into_iter().map(row_into_record).collect()
        })
    }
}

fn row_into_record(row: NotificationRow) -> Result<DatabaseNotification, NotifyError> {
    let data = serde_json::from_str(&row.data)
        .map_err(|error| NotifyError::Store(format!("deserialize notification data: {error}")))?;
    let read_at = match row.read_at {
        Some(text) => Some(decode_time(&text)?),
        None => None,
    };
    Ok(DatabaseNotification {
        id: row.id,
        notifiable_id: row.notifiable_id,
        notification_type: row.notification_type,
        data,
        read_at,
        created_at: decode_time(&row.created_at)?,
    })
}

fn to_store_error(error: &toasty::Error) -> NotifyError {
    NotifyError::Store(error.to_string())
}

/// Encode a [`SystemTime`] as a fixed-width, lexicographically sortable
/// nanoseconds-since-epoch string (the `TEXT` column stays chronologically
/// ordered under `ORDER BY`).
fn encode_time(time: SystemTime) -> String {
    let nanos = time
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or_default();
    let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
    format!("{nanos:020}")
}

fn decode_time(text: &str) -> Result<SystemTime, NotifyError> {
    let nanos: u64 = text
        .trim()
        .parse()
        .map_err(|_| NotifyError::Store(format!("invalid stored timestamp `{text}`")))?;
    Ok(UNIX_EPOCH + Duration::from_nanos(nanos))
}
