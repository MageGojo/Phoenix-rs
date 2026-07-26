//! Integration tests for the Toasty-backed [`DbNotificationStore`].
//!
//! They cover the trait contract end to end against `SQLite`: insert → unread →
//! mark-read idempotency, `data` JSON round-tripping, `read_at` semantics,
//! duplicate rejection, and "restart survival" (a fresh store over a reopened
//! file database reads previously persisted rows).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_database::{Database, TestDatabase, models};
use phoenix_notify::NotificationStore;
use phoenix_notify::{DatabaseNotification, DbNotificationStore, NotificationRow, NotifyError};
use serde_json::json;

async fn memory_store() -> DbNotificationStore {
    let database = TestDatabase::new(models!(NotificationRow))
        .await
        .expect("test database")
        .into_database();
    DbNotificationStore::new(database)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn note(
    id: &str,
    notifiable: &str,
    created_secs: u64,
    data: serde_json::Value,
) -> DatabaseNotification {
    DatabaseNotification {
        id: id.to_owned(),
        notifiable_id: notifiable.to_owned(),
        notification_type: "payment.succeeded".to_owned(),
        data,
        read_at: None,
        created_at: at(created_secs),
    }
}

#[tokio::test]
async fn insert_unread_mark_read_roundtrip_is_idempotent() {
    let store = memory_store().await;

    // Distinct created_at so ordering is deterministic (oldest first).
    store
        .insert(note("n-1", "user-1", 1, json!({ "amount": 990 })))
        .await
        .expect("insert first");
    store
        .insert(note("n-2", "user-1", 2, json!({ "amount": 5 })))
        .await
        .expect("insert second");
    // A record for a different notifiable must not leak into user-1's list.
    store
        .insert(note("n-3", "user-2", 1, json!({ "amount": 1 })))
        .await
        .expect("insert other");

    let unread = store.unread_for("user-1").await.expect("unread");
    assert_eq!(unread.len(), 2);
    assert_eq!(unread[0].id, "n-1", "oldest first");
    assert_eq!(unread[1].id, "n-2");
    assert_eq!(unread[0].created_at, at(1), "created_at round-trips");
    assert!(unread[0].read_at.is_none());
    assert!(store.unread_for("nobody").await.expect("empty").is_empty());

    // Mark the first read; read_at is stamped and it drops out of unread.
    let read_at = at(1_000);
    let marked = store.mark_read("n-1", read_at).await.expect("mark read");
    assert!(marked.is_read());
    assert_eq!(marked.read_at, Some(read_at), "read_at round-trips");

    // Idempotent: marking again with a different time keeps the original stamp.
    let again = store.mark_read("n-1", at(9_999)).await.expect("mark again");
    assert_eq!(again.read_at, Some(read_at));

    let unread = store.unread_for("user-1").await.expect("unread after read");
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].id, "n-2");
}

#[tokio::test]
async fn data_json_round_trips_through_the_text_column() {
    let store = memory_store().await;
    let payload = json!({
        "out_trade_no": "PX-1001",
        "amount": 990,
        "items": ["a", "b"],
        "nested": { "flag": true, "note": "包月" },
    });
    store
        .insert(note("n-json", "user-json", 1, payload.clone()))
        .await
        .expect("insert");

    let unread = store.unread_for("user-json").await.expect("unread");
    assert_eq!(unread.len(), 1);
    assert_eq!(
        unread[0].data, payload,
        "JSON value survives the round trip"
    );
}

#[tokio::test]
async fn duplicate_id_is_rejected() {
    let store = memory_store().await;
    store
        .insert(note("dup", "user-1", 1, json!({})))
        .await
        .expect("first insert");

    let error = store
        .insert(note("dup", "user-1", 2, json!({})))
        .await
        .expect_err("duplicate id");
    assert_eq!(
        error,
        NotifyError::DuplicateNotification {
            id: "dup".to_owned(),
        }
    );
}

#[tokio::test]
async fn mark_read_unknown_id_fails() {
    let store = memory_store().await;
    let error = store
        .mark_read("missing", at(1))
        .await
        .expect_err("unknown id");
    assert_eq!(
        error,
        NotifyError::NotificationNotFound {
            id: "missing".to_owned(),
        }
    );
}

#[tokio::test]
async fn persisted_notifications_survive_a_restart() {
    // A file database so a genuinely fresh connection (a "restart") can reopen
    // the same bytes on disk.
    let path = unique_db_path("phoenix_notify_restart");
    let url = format!("sqlite:{}", path.display());
    let _ = std::fs::remove_file(&path);

    // First "process": create the schema and persist one notification, then
    // drop the store and connection.
    {
        let database = Database::builder(models!(NotificationRow))
            .connect(&url)
            .await
            .expect("connect");
        database.initialize_schema().await.expect("schema");
        let store = DbNotificationStore::new(database);
        store
            .insert(note("survivor", "user-restart", 1, json!({ "amount": 42 })))
            .await
            .expect("insert");
    }

    // Second "process": reopen the same file with a brand-new store and read
    // the row written before the "restart".
    {
        let database = Database::builder(models!(NotificationRow))
            .connect(&url)
            .await
            .expect("reconnect");
        let store = DbNotificationStore::new(database);
        let unread = store.unread_for("user-restart").await.expect("unread");
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, "survivor");
        assert_eq!(unread[0].data, json!({ "amount": 42 }));
    }

    let _ = std::fs::remove_file(&path);
}

fn unique_db_path(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or_default();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{nanos}_{unique}.sqlite3",
        std::process::id()
    ))
}
