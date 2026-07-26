use phoenix_database::Migration;
use phoenix_plugin::{Capability, Plugin};

/// Ordered id of the `notifications` table migration.
///
/// Sorts after `phoenix-pay`'s `202607260001` so apps installing both plugins
/// (pay first) keep a strictly increasing migration list.
pub const NOTIFICATIONS_MIGRATION_ID: &str = "202607260002";

/// The `notifications` table: one row per stored database notification.
///
/// SQL targets `SQLite` first (the workspace default); `PostgreSQL` accepts
/// it, `MySQL` needs an adjusted `DROP INDEX` — revisit with the Toasty-backed
/// store (same note as the `payments` migration in `phoenix-pay`).
#[must_use]
pub fn notifications_migration() -> Migration {
    Migration::new(NOTIFICATIONS_MIGRATION_ID, "create notifications table")
        .up("CREATE TABLE IF NOT EXISTS notifications (\
             id TEXT PRIMARY KEY, \
             notifiable_id TEXT NOT NULL, \
             type TEXT NOT NULL, \
             data TEXT NOT NULL, \
             read_at TEXT, \
             created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .up("CREATE INDEX IF NOT EXISTS notifications_notifiable_read \
             ON notifications (notifiable_id, read_at)")
        .down("DROP INDEX IF EXISTS notifications_notifiable_read")
        .down("DROP TABLE IF EXISTS notifications")
}

/// Phoenix Feature installing the `notifications` migration.
///
/// Registers **no routes** on purpose: notification listing and mark-read
/// endpoints are application concerns (auth, pagination, serialization).
/// `docs/NOTIFICATIONS.md` shows a handler example built on
/// [`Notifier::unread_for`](crate::Notifier::unread_for) /
/// [`Notifier::mark_read`](crate::Notifier::mark_read).
#[derive(Clone, Copy, Debug, Default)]
pub struct NotifyFeature;

impl NotifyFeature {
    /// The feature carries no state; construct and install.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Plugin for NotifyFeature {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Migrations]
    }

    fn migrations(&self) -> Vec<Migration> {
        vec![notifications_migration()]
    }
}
