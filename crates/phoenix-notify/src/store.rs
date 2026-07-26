use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use phoenix_http::BoxFuture;

use crate::NotifyError;

/// One row of the `notifications` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseNotification {
    /// UUID primary key assigned by the [`Notifier`](crate::Notifier).
    pub id: String,
    /// Identity of the receiving [`Notifiable`](crate::Notifiable).
    pub notifiable_id: String,
    /// Stable notification type (`notifications.type`).
    pub notification_type: String,
    /// JSON payload from [`Notification::to_database`](crate::Notification::to_database).
    pub data: serde_json::Value,
    /// When the notification was marked read; `None` while unread.
    pub read_at: Option<SystemTime>,
    /// Creation timestamp.
    pub created_at: SystemTime,
}

impl DatabaseNotification {
    /// Whether the notification has been marked read.
    #[must_use]
    pub fn is_read(&self) -> bool {
        self.read_at.is_some()
    }
}

/// Persistence for the database notification channel.
///
/// The `notifications` table migration ships via [`crate::NotifyFeature`].
/// Two implementations ship: [`MemoryNotificationStore`] (tests and local
/// development) and [`DbNotificationStore`](crate::DbNotificationStore), the
/// Toasty-backed store that survives a restart. Custom stores implement this
/// trait (async style matches the rest of the workspace: [`BoxFuture`]).
pub trait NotificationStore: Send + Sync {
    /// Insert a new record; ids must be unique.
    fn insert(&self, record: DatabaseNotification) -> BoxFuture<Result<(), NotifyError>>;

    /// Set `read_at` and return the updated record. Marking an already-read
    /// notification is idempotent: the original `read_at` is kept.
    fn mark_read(
        &self,
        id: &str,
        read_at: SystemTime,
    ) -> BoxFuture<Result<DatabaseNotification, NotifyError>>;

    /// Unread records for one notifiable, oldest first.
    fn unread_for(
        &self,
        notifiable_id: &str,
    ) -> BoxFuture<Result<Vec<DatabaseNotification>, NotifyError>>;
}

/// Thread-safe in-memory [`NotificationStore`] for tests and local development.
#[derive(Clone, Default)]
pub struct MemoryNotificationStore {
    records: Arc<Mutex<Vec<DatabaseNotification>>>,
}

impl MemoryNotificationStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of every stored record (insertion order).
    #[must_use]
    pub fn all(&self) -> Vec<DatabaseNotification> {
        self.lock().clone()
    }

    /// Number of stored records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the store holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<DatabaseNotification>> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for MemoryNotificationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryNotificationStore")
            .field("records", &self.len())
            .finish()
    }
}

impl NotificationStore for MemoryNotificationStore {
    fn insert(&self, record: DatabaseNotification) -> BoxFuture<Result<(), NotifyError>> {
        let result = {
            let mut records = self.lock();
            if records.iter().any(|existing| existing.id == record.id) {
                Err(NotifyError::DuplicateNotification { id: record.id })
            } else {
                records.push(record);
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn mark_read(
        &self,
        id: &str,
        read_at: SystemTime,
    ) -> BoxFuture<Result<DatabaseNotification, NotifyError>> {
        let result = {
            let mut records = self.lock();
            match records.iter_mut().find(|record| record.id == id) {
                Some(record) => {
                    if record.read_at.is_none() {
                        record.read_at = Some(read_at);
                    }
                    Ok(record.clone())
                }
                None => Err(NotifyError::NotificationNotFound { id: id.to_owned() }),
            }
        };
        Box::pin(async move { result })
    }

    fn unread_for(
        &self,
        notifiable_id: &str,
    ) -> BoxFuture<Result<Vec<DatabaseNotification>, NotifyError>> {
        let result = Ok(self
            .lock()
            .iter()
            .filter(|record| record.notifiable_id == notifiable_id && !record.is_read())
            .cloned()
            .collect());
        Box::pin(async move { result })
    }
}
