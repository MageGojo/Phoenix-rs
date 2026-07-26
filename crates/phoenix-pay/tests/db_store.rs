//! Integration tests for the Toasty-backed [`DbPaymentStore`].
//!
//! They cover the trait contract end to end against `SQLite`: insert / find
//! (hit and miss), the full state machine (legal and illegal transitions),
//! the `(provider, out_trade_no)` unique constraint, idempotent notify replays
//! through [`PayManager`], and "restart survival" (a fresh store over a
//! reopened file database reads previously persisted orders).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_database::{Database, TestDatabase, models};
use phoenix_pay::PaymentStore;
use phoenix_pay::prelude::*;

async fn memory_store() -> DbPaymentStore {
    let database = TestDatabase::new(models!(PaymentRow))
        .await
        .expect("test database")
        .into_database();
    DbPaymentStore::new(database)
}

fn order(out_trade_no: &str) -> PaymentRecord {
    PaymentRecord::new(
        "mock",
        &CreateOrder::new(out_trade_no, Amount::cny(1234), "会员月卡"),
        UNIX_EPOCH + Duration::from_hours(500_000),
    )
}

#[tokio::test]
async fn insert_find_hit_and_miss() {
    let store = memory_store().await;
    store.insert(order("T100")).await.expect("insert");

    let found = store
        .find("mock", "T100")
        .await
        .expect("find")
        .expect("hit");
    assert_eq!(found, order("T100"));
    assert_eq!(found.status, PaymentStatus::Created);
    assert_eq!(found.amount, Amount::cny(1234));

    assert!(store.find("mock", "NOPE").await.expect("miss").is_none());
    assert!(store.find("other", "T100").await.expect("miss").is_none());
}

#[tokio::test]
async fn full_state_machine_legal_and_illegal() {
    let store = memory_store().await;
    store.insert(order("T200")).await.expect("insert");

    // Legal: Created -> Pending -> Paid, notify payload persisted on Paid.
    let pending = store
        .transition("mock", "T200", PaymentStatus::Pending, None)
        .await
        .expect("to pending");
    assert_eq!(pending.status, PaymentStatus::Pending);

    let paid = store
        .transition(
            "mock",
            "T200",
            PaymentStatus::Paid,
            Some("raw-notify".to_owned()),
        )
        .await
        .expect("to paid");
    assert_eq!(paid.status, PaymentStatus::Paid);
    assert_eq!(paid.notify_payload.as_deref(), Some("raw-notify"));

    // The payload persists and is visible through find.
    let stored = store
        .find("mock", "T200")
        .await
        .expect("find")
        .expect("hit");
    assert_eq!(stored.status, PaymentStatus::Paid);
    assert_eq!(stored.notify_payload.as_deref(), Some("raw-notify"));

    // Illegal: Paid -> Failed, and a same-status no-op, both rejected; state
    // stays Paid.
    assert!(matches!(
        store
            .transition("mock", "T200", PaymentStatus::Failed, None)
            .await,
        Err(PayError::InvalidTransition {
            from: PaymentStatus::Paid,
            to: PaymentStatus::Failed,
        })
    ));
    assert!(matches!(
        store
            .transition("mock", "T200", PaymentStatus::Paid, None)
            .await,
        Err(PayError::InvalidTransition { .. })
    ));
    assert_eq!(
        store.find("mock", "T200").await.unwrap().unwrap().status,
        PaymentStatus::Paid
    );
}

#[tokio::test]
async fn created_can_close_but_not_jump_to_paid() {
    let store = memory_store().await;
    store.insert(order("T210")).await.expect("insert");

    // Created -> Paid is illegal (must go through Pending).
    assert!(matches!(
        store
            .transition("mock", "T210", PaymentStatus::Paid, None)
            .await,
        Err(PayError::InvalidTransition { .. })
    ));
    // Created -> Closed is legal (auditable parked order).
    let closed = store
        .transition("mock", "T210", PaymentStatus::Closed, None)
        .await
        .expect("to closed");
    assert_eq!(closed.status, PaymentStatus::Closed);
}

#[tokio::test]
async fn duplicate_provider_out_trade_no_conflicts() {
    let store = memory_store().await;
    store.insert(order("T300")).await.expect("insert");

    let error = store.insert(order("T300")).await.expect_err("duplicate");
    assert_eq!(
        error,
        PayError::DuplicateOrder {
            provider: "mock".to_owned(),
            out_trade_no: "T300".to_owned(),
        }
    );

    // The same out_trade_no under a different provider is a distinct order.
    let other = PaymentRecord::new(
        "wechat_native",
        &CreateOrder::new("T300", Amount::cny(1), "x"),
        SystemTime::now(),
    );
    store.insert(other).await.expect("distinct provider");
}

#[tokio::test]
async fn transition_unknown_order_fails() {
    let store = memory_store().await;
    let error = store
        .transition("mock", "GHOST", PaymentStatus::Pending, None)
        .await
        .expect_err("unknown order");
    assert_eq!(
        error,
        PayError::OrderNotFound {
            provider: "mock".to_owned(),
            out_trade_no: "GHOST".to_owned(),
        }
    );
}

#[tokio::test]
async fn manager_notify_is_idempotent_over_the_db_store() {
    let database = TestDatabase::new(models!(PaymentRow))
        .await
        .expect("test database")
        .into_database();
    let provider = MockProvider::new();
    let manager = PayManager::builder()
        .provider(Arc::new(provider.clone()))
        .store(Arc::new(DbPaymentStore::new(database)))
        .build();

    manager
        .create(
            "mock",
            CreateOrder::new("T400", Amount::cny(1234), "会员月卡"),
        )
        .await
        .expect("create");
    let body = provider.mark_paid("T400").expect("mark paid");

    let first = manager
        .handle_notify("mock", NotifyRequest::from_body(body.clone()))
        .await
        .expect("first notify");
    assert!(matches!(first, NotifyOutcome::Processed(_)));

    for _ in 0..3 {
        let replay = manager
            .handle_notify("mock", NotifyRequest::from_body(body.clone()))
            .await
            .expect("replay");
        assert!(
            matches!(replay, NotifyOutcome::AlreadyProcessed(_)),
            "duplicate notify must not transition a second time"
        );
    }

    let stored = manager.find_order("mock", "T400").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Paid);
}

#[tokio::test]
async fn persisted_orders_survive_a_restart() {
    let path = unique_db_path("phoenix_pay_restart");
    let url = format!("sqlite:{}", path.display());
    let _ = std::fs::remove_file(&path);

    // First "process": persist a paid order, then drop the connection.
    {
        let database = Database::builder(models!(PaymentRow))
            .connect(&url)
            .await
            .expect("connect");
        database.initialize_schema().await.expect("schema");
        let store = DbPaymentStore::new(database);
        store.insert(order("T500")).await.expect("insert");
        store
            .transition("mock", "T500", PaymentStatus::Pending, None)
            .await
            .expect("pending");
        store
            .transition("mock", "T500", PaymentStatus::Paid, Some("raw".to_owned()))
            .await
            .expect("paid");
    }

    // Second "process": reopen and confirm the order and its status survived.
    {
        let database = Database::builder(models!(PaymentRow))
            .connect(&url)
            .await
            .expect("reconnect");
        let store = DbPaymentStore::new(database);
        let found = store
            .find("mock", "T500")
            .await
            .expect("find")
            .expect("hit");
        assert_eq!(found.status, PaymentStatus::Paid);
        assert_eq!(found.amount, Amount::cny(1234));
        assert_eq!(found.notify_payload.as_deref(), Some("raw"));

        // A further legal transition still works against the reopened database.
        let refunding = store
            .transition("mock", "T500", PaymentStatus::Refunding, None)
            .await
            .expect("refunding");
        assert_eq!(refunding.status, PaymentStatus::Refunding);
    }

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn paid_at_is_stamped_once_and_bounds_the_reconciliation_window() {
    let store = memory_store().await;
    store.insert(order("T600")).await.expect("insert");
    let before = SystemTime::now();

    assert!(
        store
            .find("mock", "T600")
            .await
            .unwrap()
            .unwrap()
            .paid_at
            .is_none(),
        "an unpaid order has no paid_at"
    );

    store
        .transition("mock", "T600", PaymentStatus::Pending, None)
        .await
        .expect("pending");
    let paid = store
        .transition("mock", "T600", PaymentStatus::Paid, None)
        .await
        .expect("paid");
    let stamped = paid.paid_at.expect("paid_at is stamped on the Paid move");
    assert!(stamped >= before);

    // A refund that fails walks Paid -> Refunding -> Paid; the original
    // settlement day must not move with it.
    store
        .transition("mock", "T600", PaymentStatus::Refunding, None)
        .await
        .expect("refunding");
    let back = store
        .transition("mock", "T600", PaymentStatus::Paid, None)
        .await
        .expect("paid again");
    assert_eq!(
        back.paid_at,
        Some(stamped),
        "paid_at is stamped exactly once"
    );

    let window = store
        .paid_within(
            "mock",
            stamped - Duration::from_secs(1),
            stamped + Duration::from_secs(1),
        )
        .await
        .expect("paid_within");
    assert_eq!(window.len(), 1);
    assert_eq!(window[0].out_trade_no, "T600");

    // Half-open window: `to` is exclusive, so an order paid exactly at the
    // boundary belongs to the next day, not this one.
    assert!(
        store
            .paid_within("mock", stamped - Duration::from_secs(1), stamped)
            .await
            .expect("paid_within")
            .is_empty()
    );
    assert!(
        store
            .paid_within(
                "wechat_native",
                stamped - Duration::from_secs(1),
                stamped + Duration::from_secs(1)
            )
            .await
            .expect("paid_within")
            .is_empty(),
        "the window is provider-scoped"
    );
}

#[tokio::test]
async fn refunds_persist_with_their_own_idempotency_key() {
    let database = TestDatabase::new(models!(PaymentRow, RefundRow))
        .await
        .expect("test database")
        .into_database();
    let store = DbPaymentStore::new(database);

    let created_at = UNIX_EPOCH + Duration::from_hours(500_000);
    let request = RefundOrder::partial("T700", "R-1", Amount::cny(300), Amount::cny(1234))
        .reason("尺码不合适");
    store
        .insert_refund(RefundRecord::new("mock", &request, created_at))
        .await
        .expect("insert refund");

    let found = store
        .find_refund("mock", "R-1")
        .await
        .expect("find")
        .expect("hit");
    assert_eq!(found.out_trade_no, "T700");
    assert_eq!(found.amount, Amount::cny(300));
    assert_eq!(found.status, RefundStatus::Processing);
    assert_eq!(found.reason.as_deref(), Some("尺码不合适"));
    assert_eq!(found.created_at, created_at);
    assert!(found.refund_id.is_none());
    assert!(store.find_refund("mock", "NOPE").await.unwrap().is_none());

    // The same refund number is rejected, exactly like a duplicate order.
    assert_eq!(
        store
            .insert_refund(RefundRecord::new("mock", &request, created_at))
            .await,
        Err(PayError::DuplicateRefund {
            provider: "mock".to_owned(),
            out_refund_no: "R-1".to_owned(),
        })
    );

    // Recording the provider id leaves the status alone.
    store
        .record_refund_id("mock", "R-1", "50000-R-1")
        .await
        .expect("record id");
    let found = store.find_refund("mock", "R-1").await.unwrap().unwrap();
    assert_eq!(found.refund_id.as_deref(), Some("50000-R-1"));
    assert_eq!(found.status, RefundStatus::Processing);

    // A second refund on the same order, listed oldest first.
    let second = RefundOrder::partial("T700", "R-2", Amount::cny(934), Amount::cny(1234));
    store
        .insert_refund(RefundRecord::new(
            "mock",
            &second,
            created_at + Duration::from_mins(1),
        ))
        .await
        .expect("insert second");
    let listed = store.refunds_for("mock", "T700").await.expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|refund| refund.out_refund_no.as_str())
            .collect::<Vec<_>>(),
        vec!["R-1", "R-2"]
    );
    assert!(
        store
            .refunds_for("mock", "OTHER")
            .await
            .expect("list")
            .is_empty()
    );

    let settled = store
        .transition_refund("mock", "R-1", RefundStatus::Succeeded, None)
        .await
        .expect("settle");
    assert_eq!(settled.status, RefundStatus::Succeeded);
    assert_eq!(settled.refund_id.as_deref(), Some("50000-R-1"));

    // Terminal refunds do not reopen.
    assert_eq!(
        store
            .transition_refund("mock", "R-1", RefundStatus::Failed, None)
            .await,
        Err(PayError::InvalidRefundTransition {
            from: RefundStatus::Succeeded,
            to: RefundStatus::Failed,
        })
    );
    assert_eq!(
        store
            .transition_refund("mock", "GHOST", RefundStatus::Failed, None)
            .await,
        Err(PayError::RefundNotFound {
            provider: "mock".to_owned(),
            out_refund_no: "GHOST".to_owned(),
        })
    );
}

fn unique_db_path(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
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
