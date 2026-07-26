//! End-to-end refund and reconciliation tests through [`PayManager`].
//!
//! They pin the behaviour that actually protects money:
//!
//! - a refund is persisted **before** the provider is called, so a crash mid
//!   call leaves an auditable row rather than a silent loss;
//! - the same `out_refund_no` never charges twice;
//! - partial refunds accumulate and cannot exceed the order total, including
//!   while an earlier one is still in flight;
//! - a failed refund releases its amount so a retry is possible;
//! - the order's own status follows its refunds (`Paid -> Refunding ->
//!   Refunded`, and back to `Paid` when everything failed);
//! - reconciliation reports both directions of disagreement against a bill.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use phoenix_http::BoxFuture;
use phoenix_pay::PaymentStore;
use phoenix_pay::prelude::*;

fn manager_with(provider: Arc<dyn PaymentProvider>) -> (Arc<PayManager>, Arc<MemoryPaymentStore>) {
    let store = Arc::new(MemoryPaymentStore::new());
    let manager = PayManager::builder()
        .provider(provider)
        .store(Arc::clone(&store) as Arc<dyn PaymentStore>)
        .build();
    (Arc::new(manager), store)
}

fn mock_manager() -> (Arc<PayManager>, MockProvider, Arc<MemoryPaymentStore>) {
    let provider = MockProvider::new();
    let (manager, store) = manager_with(Arc::new(provider.clone()));
    (manager, provider, store)
}

/// Create and pay one order of 12.34 CNY.
async fn paid_order(manager: &PayManager, provider: &MockProvider, out_trade_no: &str) {
    manager
        .create(
            "mock",
            CreateOrder::new(out_trade_no, Amount::cny(1234), "会员月卡"),
        )
        .await
        .expect("create");
    let body = provider.mark_paid(out_trade_no).expect("mark paid");
    manager
        .handle_notify("mock", NotifyRequest::from_body(body))
        .await
        .expect("notify");
}

#[tokio::test]
async fn a_full_refund_walks_the_order_to_refunded() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T100").await;

    let receipt = manager
        .refund("mock", RefundOrder::full("T100", "R-1", Amount::cny(1234)))
        .await
        .expect("refund");
    assert_eq!(receipt.status, RefundStatus::Succeeded);
    assert_eq!(receipt.amount, Amount::cny(1234));

    let stored = manager.find_order("mock", "T100").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Refunded);

    let refunds = manager.refunds_for("mock", "T100").await.expect("refunds");
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].out_refund_no, "R-1");
    assert_eq!(refunds[0].status, RefundStatus::Succeeded);
    assert_eq!(refunds[0].refund_id.as_deref(), Some("MOCK-REFUND-R-1"));
}

#[tokio::test]
async fn partial_refunds_accumulate_and_cannot_over_refund() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T200").await;

    manager
        .refund(
            "mock",
            RefundOrder::partial("T200", "R-1", Amount::cny(1000), Amount::cny(1234)),
        )
        .await
        .expect("first partial");
    let stored = manager.find_order("mock", "T200").await.unwrap().unwrap();
    assert_eq!(
        stored.status,
        PaymentStatus::Refunding,
        "a partial refund leaves the order refunding, not refunded"
    );

    // 10.00 + 2.35 > 12.34: rejected before the provider is called.
    let error = manager
        .refund(
            "mock",
            RefundOrder::partial("T200", "R-2", Amount::cny(235), Amount::cny(1234)),
        )
        .await
        .expect_err("over-refund must be rejected");
    assert!(
        matches!(
            &error,
            PayError::RefundExceedsOrder { refundable, .. } if *refundable == Amount::cny(234)
        ),
        "unexpected error: {error:?}"
    );

    manager
        .refund(
            "mock",
            RefundOrder::partial("T200", "R-3", Amount::cny(234), Amount::cny(1234)),
        )
        .await
        .expect("remainder");
    let stored = manager.find_order("mock", "T200").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Refunded);
    assert_eq!(manager.refunds_for("mock", "T200").await.unwrap().len(), 2);
}

#[tokio::test]
async fn the_same_refund_number_never_charges_twice() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T300").await;

    let first = manager
        .refund(
            "mock",
            RefundOrder::partial("T300", "R-1", Amount::cny(600), Amount::cny(1234)),
        )
        .await
        .expect("first");
    let replay = manager
        .refund(
            "mock",
            RefundOrder::partial("T300", "R-1", Amount::cny(600), Amount::cny(1234)),
        )
        .await
        .expect("replay");

    assert_eq!(replay.out_refund_no, first.out_refund_no);
    assert_eq!(replay.amount, first.amount);
    assert_eq!(replay.status, first.status);
    assert_eq!(
        manager.refunds_for("mock", "T300").await.unwrap().len(),
        1,
        "a replay must not create a second refund row"
    );

    // The remaining 6.34 is still refundable — the replay did not consume it.
    manager
        .refund(
            "mock",
            RefundOrder::partial("T300", "R-2", Amount::cny(634), Amount::cny(1234)),
        )
        .await
        .expect("remainder");
    let stored = manager.find_order("mock", "T300").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Refunded);
}

#[tokio::test]
async fn reusing_a_refund_number_across_orders_is_rejected() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T400").await;
    paid_order(&manager, &provider, "T401").await;

    manager
        .refund("mock", RefundOrder::full("T400", "R-1", Amount::cny(1234)))
        .await
        .expect("first order");
    let error = manager
        .refund("mock", RefundOrder::full("T401", "R-1", Amount::cny(1234)))
        .await
        .expect_err("shared refund number must be rejected");
    assert!(
        matches!(&error, PayError::InvalidRefund(message) if message.contains("different order")),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn an_in_flight_refund_holds_its_amount_until_it_settles() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T500").await;
    provider.defer_refund("R-SLOW");

    let receipt = manager
        .refund(
            "mock",
            RefundOrder::partial("T500", "R-SLOW", Amount::cny(1000), Amount::cny(1234)),
        )
        .await
        .expect("slow refund");
    assert_eq!(receipt.status, RefundStatus::Processing);

    let stored = manager.find_order("mock", "T500").await.unwrap().unwrap();
    assert_eq!(
        stored.status,
        PaymentStatus::Refunding,
        "an unsettled refund still moves the order out of Paid"
    );

    // The pending 10.00 is reserved: a second refund cannot spend it.
    assert!(matches!(
        manager
            .refund(
                "mock",
                RefundOrder::partial("T500", "R-2", Amount::cny(300), Amount::cny(1234)),
            )
            .await,
        Err(PayError::RefundExceedsOrder { .. })
    ));

    // Polling before the provider settles changes nothing.
    let polled = manager.sync_refund("mock", "R-SLOW").await.expect("poll");
    assert_eq!(polled.status, RefundStatus::Processing);

    provider
        .settle_refund("R-SLOW", RefundStatus::Succeeded)
        .expect("settle");
    let settled = manager.sync_refund("mock", "R-SLOW").await.expect("poll");
    assert_eq!(settled.status, RefundStatus::Succeeded);

    // A settled refund is terminal: re-polling is a no-op, not an error.
    assert_eq!(
        manager.sync_refund("mock", "R-SLOW").await.unwrap().status,
        RefundStatus::Succeeded
    );

    let stored = manager.find_order("mock", "T500").await.unwrap().unwrap();
    assert_eq!(stored.status, PaymentStatus::Refunding);
}

#[tokio::test]
async fn a_failed_refund_releases_its_amount_and_restores_the_order() {
    let (manager, provider, _store) = mock_manager();
    paid_order(&manager, &provider, "T600").await;
    provider.defer_refund("R-BAD");

    manager
        .refund(
            "mock",
            RefundOrder::full("T600", "R-BAD", Amount::cny(1234)),
        )
        .await
        .expect("accepted");
    provider
        .settle_refund("R-BAD", RefundStatus::Failed)
        .expect("settle");
    let failed = manager.sync_refund("mock", "R-BAD").await.expect("poll");
    assert_eq!(failed.status, RefundStatus::Failed);

    let stored = manager.find_order("mock", "T600").await.unwrap().unwrap();
    assert_eq!(
        stored.status,
        PaymentStatus::Paid,
        "when every refund failed, the order is paid again"
    );

    // The full amount is refundable once more, under a new number.
    let retry = manager
        .refund(
            "mock",
            RefundOrder::full("T600", "R-RETRY", Amount::cny(1234)),
        )
        .await
        .expect("retry");
    assert_eq!(retry.status, RefundStatus::Succeeded);
    assert_eq!(
        manager
            .find_order("mock", "T600")
            .await
            .unwrap()
            .unwrap()
            .status,
        PaymentStatus::Refunded
    );
}

#[tokio::test]
async fn unpaid_and_unknown_orders_cannot_be_refunded() {
    let (manager, provider, _store) = mock_manager();
    manager
        .create("mock", CreateOrder::new("T700", Amount::cny(1234), "x"))
        .await
        .expect("create");

    assert!(matches!(
        manager
            .refund("mock", RefundOrder::full("T700", "R-1", Amount::cny(1234)))
            .await,
        Err(PayError::InvalidRefund(_))
    ));
    assert!(matches!(
        manager
            .refund("mock", RefundOrder::full("GHOST", "R-2", Amount::cny(1)))
            .await,
        Err(PayError::OrderNotFound { .. })
    ));

    // A total that disagrees with the stored order is a caller bug, not a
    // silent partial refund.
    paid_order(&manager, &provider, "T701").await;
    let error = manager
        .refund(
            "mock",
            RefundOrder::partial("T701", "R-3", Amount::cny(1), Amount::cny(9999)),
        )
        .await
        .expect_err("mismatched total");
    assert!(
        matches!(&error, PayError::InvalidRefund(message) if message.contains("total")),
        "unexpected error: {error:?}"
    );
}

/// A provider whose refund call always fails at the gateway.
struct BrokenRefundProvider(MockProvider);

impl PaymentProvider for BrokenRefundProvider {
    fn key(&self) -> &'static str {
        MockProvider::KEY
    }

    fn create(&self, order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>> {
        self.0.create(order)
    }

    fn verify_notify(&self, notify: &NotifyRequest) -> BoxFuture<Result<NotifyEvent, PayError>> {
        self.0.verify_notify(notify)
    }

    fn query(&self, out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>> {
        self.0.query(out_trade_no)
    }

    fn refund(&self, _refund: &RefundOrder) -> BoxFuture<Result<RefundReceipt, PayError>> {
        Box::pin(async { Err(PayError::Gateway("connection reset".to_owned())) })
    }
}

#[tokio::test]
async fn a_provider_error_leaves_an_auditable_failed_refund() {
    let inner = MockProvider::new();
    let (manager, _store) = manager_with(Arc::new(BrokenRefundProvider(inner.clone())));
    manager
        .create("mock", CreateOrder::new("T800", Amount::cny(1234), "x"))
        .await
        .expect("create");
    let body = inner.mark_paid("T800").expect("mark paid");
    manager
        .handle_notify("mock", NotifyRequest::from_body(body))
        .await
        .expect("notify");

    let error = manager
        .refund("mock", RefundOrder::full("T800", "R-1", Amount::cny(1234)))
        .await
        .expect_err("gateway error");
    assert!(matches!(&error, PayError::Gateway(_)), "{error:?}");

    // The attempt is on record, marked failed, and its amount is released.
    let refunds = manager.refunds_for("mock", "T800").await.expect("refunds");
    assert_eq!(refunds.len(), 1);
    assert_eq!(refunds[0].status, RefundStatus::Failed);
    assert_eq!(
        manager
            .find_order("mock", "T800")
            .await
            .unwrap()
            .unwrap()
            .status,
        PaymentStatus::Paid
    );
}

#[tokio::test]
async fn reconciling_a_day_matches_the_bill_against_local_orders() {
    let (manager, provider, store) = mock_manager();
    let window_start = SystemTime::now() - Duration::from_mins(1);
    paid_order(&manager, &provider, "T-A").await;
    paid_order(&manager, &provider, "T-B").await;

    // A local order the provider never settled: still Pending, so it is not
    // expected in the bill and must not be reported.
    manager
        .create("mock", CreateOrder::new("T-C", Amount::cny(1), "x"))
        .await
        .expect("create");

    let balanced = manager
        .reconcile_day("mock", "2026-07-25", window_start)
        .await
        .expect("reconcile");
    assert!(
        balanced.is_balanced(),
        "unexpected discrepancies: {:?}",
        balanced.discrepancies
    );
    assert_eq!(balanced.matched, 2);
    assert_eq!(balanced.date, "2026-07-25");

    // Now make the two sides disagree: the bill carries an order we never
    // recorded, and one whose amount does not match.
    let mut bill = Bill {
        provider: "mock".to_owned(),
        date: "2026-07-25".to_owned(),
        entries: vec![
            BillEntry {
                out_trade_no: "T-A".to_owned(),
                transaction_id: None,
                amount: Amount::cny(9999),
                refunded: Amount::cny(0),
                status: PaymentStatus::Paid,
            },
            BillEntry {
                out_trade_no: "T-GHOST".to_owned(),
                transaction_id: None,
                amount: Amount::cny(500),
                refunded: Amount::cny(0),
                status: PaymentStatus::Paid,
            },
        ],
    };
    let result = manager
        .reconcile_bill(&bill, window_start)
        .await
        .expect("reconcile");
    assert!(!result.is_balanced());
    assert_eq!(
        result.discrepancies,
        vec![
            Discrepancy::AmountMismatch {
                out_trade_no: "T-A".to_owned(),
                local: Amount::cny(1234),
                remote: Amount::cny(9999),
            },
            // T-B is paid locally but absent from this bill.
            Discrepancy::MissingRemotely {
                out_trade_no: "T-B".to_owned(),
                amount: Amount::cny(1234),
            },
            Discrepancy::MissingLocally {
                out_trade_no: "T-GHOST".to_owned(),
                amount: Amount::cny(500),
            },
        ]
    );

    // A window that does not contain the payments finds nothing local, so
    // every settled line is reported as missing locally.
    bill.entries.truncate(1);
    let elsewhere = manager
        .reconcile_bill(&bill, window_start - Duration::from_hours(48))
        .await
        .expect("reconcile");
    assert_eq!(
        elsewhere.discrepancies,
        vec![Discrepancy::MissingLocally {
            out_trade_no: "T-A".to_owned(),
            amount: Amount::cny(9999),
        }]
    );

    // Sanity: paid_within is what defines "the day", and it is provider-scoped.
    let paid = store
        .paid_within(
            "mock",
            window_start,
            window_start + Duration::from_hours(24),
        )
        .await
        .expect("paid_within");
    assert_eq!(paid.len(), 2);
    assert!(paid.iter().all(|record| record.paid_at.is_some()));
    assert!(
        store
            .paid_within(
                "other",
                window_start,
                window_start + Duration::from_hours(24)
            )
            .await
            .expect("paid_within")
            .is_empty()
    );
}

#[tokio::test]
async fn a_refunded_order_still_agrees_with_the_bill_that_paid_it() {
    let (manager, provider, _store) = mock_manager();
    let window_start = SystemTime::now() - Duration::from_mins(1);
    paid_order(&manager, &provider, "T-R").await;
    manager
        .refund("mock", RefundOrder::full("T-R", "R-1", Amount::cny(1234)))
        .await
        .expect("refund");

    let bill = Bill {
        provider: "mock".to_owned(),
        date: "2026-07-25".to_owned(),
        entries: vec![BillEntry {
            out_trade_no: "T-R".to_owned(),
            transaction_id: None,
            amount: Amount::cny(1234),
            refunded: Amount::cny(1234),
            status: PaymentStatus::Paid,
        }],
    };
    let result = manager
        .reconcile_bill(&bill, window_start)
        .await
        .expect("reconcile");
    assert!(
        result.is_balanced(),
        "a refund settles later than the payment: {:?}",
        result.discrepancies
    );
    assert_eq!(bill.net_total().expect("net"), Amount::cny(0));
}

#[tokio::test]
async fn providers_without_a_refund_api_say_so() {
    /// The minimum a provider must implement — no refund, no bill.
    struct Minimal;

    impl PaymentProvider for Minimal {
        fn key(&self) -> &'static str {
            "minimal"
        }

        fn create(&self, _order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>> {
            Box::pin(async { Err(PayError::NotImplemented("create")) })
        }

        fn verify_notify(
            &self,
            _notify: &NotifyRequest,
        ) -> BoxFuture<Result<NotifyEvent, PayError>> {
            Box::pin(async { Err(PayError::NotImplemented("verify_notify")) })
        }

        fn query(&self, _out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>> {
            Box::pin(async { Err(PayError::NotImplemented("query")) })
        }
    }

    let provider = Minimal;
    assert!(matches!(
        provider
            .refund(&RefundOrder::full("T", "R", Amount::cny(1)))
            .await,
        Err(PayError::NotImplemented(_))
    ));
    assert!(matches!(
        provider.query_refund("T", "R").await,
        Err(PayError::NotImplemented(_))
    ));
    assert!(matches!(
        provider.download_bill("2026-07-25").await,
        Err(PayError::NotImplemented(_))
    ));
}
