use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use phoenix_http::BoxFuture;

use crate::{PayError, PaymentRecord, PaymentStatus, RefundRecord, RefundStatus};

/// Persistence for payment orders, keyed by `(provider, out_trade_no)`, and
/// their refunds, keyed by `(provider, out_refund_no)`.
///
/// The `payments` and `payment_refunds` table migrations ship via
/// [`crate::PayFeature`]. Two implementations ship: [`MemoryPaymentStore`]
/// (tests and local development) and [`DbPaymentStore`](crate::DbPaymentStore),
/// the Toasty-backed store that survives a restart. Custom stores implement
/// this trait (async style matches the rest of the workspace: [`BoxFuture`]).
pub trait PaymentStore: Send + Sync {
    /// Insert a new record; the key must be unique.
    fn insert(&self, record: PaymentRecord) -> BoxFuture<Result<(), PayError>>;

    /// Fetch one record by key.
    fn find(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Option<PaymentRecord>, PayError>>;

    /// Atomically apply a state-machine transition (and optionally record the
    /// verified notify payload), returning the updated record.
    ///
    /// Implementations stamp [`PaymentRecord::paid_at`] the first time an order
    /// reaches [`PaymentStatus::Paid`], and never overwrite it afterwards.
    fn transition(
        &self,
        provider: &str,
        out_trade_no: &str,
        next: PaymentStatus,
        notify_payload: Option<String>,
    ) -> BoxFuture<Result<PaymentRecord, PayError>>;

    /// Orders whose `paid_at` falls in `[from, to)`, oldest first.
    ///
    /// This is the local side of daily reconciliation: pass the provider's
    /// bill day and hand the result to [`reconcile`](crate::reconcile).
    /// Orders that were paid and later refunded are still returned — the trade
    /// bill for the day of payment still lists them.
    fn paid_within(
        &self,
        provider: &str,
        from: SystemTime,
        to: SystemTime,
    ) -> BoxFuture<Result<Vec<PaymentRecord>, PayError>>;

    /// Insert a refund record; `(provider, out_refund_no)` must be unique.
    fn insert_refund(&self, record: RefundRecord) -> BoxFuture<Result<(), PayError>>;

    /// Fetch one refund by `(provider, out_refund_no)`.
    fn find_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<Option<RefundRecord>, PayError>>;

    /// Every refund recorded against one order, oldest first.
    fn refunds_for(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Vec<RefundRecord>, PayError>>;

    /// Atomically apply a refund state-machine transition, optionally recording
    /// the provider-side refund id, and return the updated record.
    fn transition_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
        next: RefundStatus,
        refund_id: Option<String>,
    ) -> BoxFuture<Result<RefundRecord, PayError>>;

    /// Record the provider-side refund id without touching the status.
    ///
    /// Needed for refunds the provider accepted but has not settled: the id is
    /// what a later [`Self::transition_refund`] poll is keyed on, and it must
    /// be durable before that poll happens.
    fn record_refund_id(
        &self,
        provider: &str,
        out_refund_no: &str,
        refund_id: &str,
    ) -> BoxFuture<Result<(), PayError>>;
}

/// Thread-safe in-memory [`PaymentStore`] for tests and local development.
#[derive(Clone, Default)]
pub struct MemoryPaymentStore {
    records: Arc<Mutex<BTreeMap<(String, String), PaymentRecord>>>,
    refunds: Arc<Mutex<BTreeMap<(String, String), RefundRecord>>>,
}

impl MemoryPaymentStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Number of stored refunds.
    #[must_use]
    pub fn refund_count(&self) -> usize {
        self.lock_refunds().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), PaymentRecord>> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_refunds(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), RefundRecord>> {
        self.refunds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for MemoryPaymentStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryPaymentStore")
            .field("records", &self.len())
            .finish()
    }
}

impl PaymentStore for MemoryPaymentStore {
    fn insert(&self, record: PaymentRecord) -> BoxFuture<Result<(), PayError>> {
        let result = {
            let mut records = self.lock();
            let key = (record.provider.clone(), record.out_trade_no.clone());
            match records.entry(key) {
                std::collections::btree_map::Entry::Occupied(_) => Err(PayError::DuplicateOrder {
                    provider: record.provider.clone(),
                    out_trade_no: record.out_trade_no.clone(),
                }),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(record);
                    Ok(())
                }
            }
        };
        Box::pin(async move { result })
    }

    fn find(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Option<PaymentRecord>, PayError>> {
        let result = Ok(self
            .lock()
            .get(&(provider.to_owned(), out_trade_no.to_owned()))
            .cloned());
        Box::pin(async move { result })
    }

    fn transition(
        &self,
        provider: &str,
        out_trade_no: &str,
        next: PaymentStatus,
        notify_payload: Option<String>,
    ) -> BoxFuture<Result<PaymentRecord, PayError>> {
        let result = {
            let mut records = self.lock();
            let key = (provider.to_owned(), out_trade_no.to_owned());
            match records.get_mut(&key) {
                Some(record) => record.status.transition(next).map(|status| {
                    record.status = status;
                    // Stamped once: a later Refunding -> Paid must not move the
                    // order into a different reconciliation day.
                    if status == PaymentStatus::Paid && record.paid_at.is_none() {
                        record.paid_at = Some(SystemTime::now());
                    }
                    if notify_payload.is_some() {
                        record.notify_payload = notify_payload;
                    }
                    record.clone()
                }),
                None => Err(PayError::OrderNotFound {
                    provider: provider.to_owned(),
                    out_trade_no: out_trade_no.to_owned(),
                }),
            }
        };
        Box::pin(async move { result })
    }

    fn paid_within(
        &self,
        provider: &str,
        from: SystemTime,
        to: SystemTime,
    ) -> BoxFuture<Result<Vec<PaymentRecord>, PayError>> {
        let mut found: Vec<PaymentRecord> = self
            .lock()
            .values()
            .filter(|record| record.provider == provider)
            .filter(|record| {
                record
                    .paid_at
                    .is_some_and(|paid_at| paid_at >= from && paid_at < to)
            })
            .cloned()
            .collect();
        found.sort_by_key(|record| record.paid_at);
        Box::pin(async move { Ok(found) })
    }

    fn insert_refund(&self, record: RefundRecord) -> BoxFuture<Result<(), PayError>> {
        let result = {
            let mut refunds = self.lock_refunds();
            let key = (record.provider.clone(), record.out_refund_no.clone());
            match refunds.entry(key) {
                std::collections::btree_map::Entry::Occupied(_) => Err(PayError::DuplicateRefund {
                    provider: record.provider.clone(),
                    out_refund_no: record.out_refund_no.clone(),
                }),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(record);
                    Ok(())
                }
            }
        };
        Box::pin(async move { result })
    }

    fn find_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<Option<RefundRecord>, PayError>> {
        let result = Ok(self
            .lock_refunds()
            .get(&(provider.to_owned(), out_refund_no.to_owned()))
            .cloned());
        Box::pin(async move { result })
    }

    fn refunds_for(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Vec<RefundRecord>, PayError>> {
        let mut found: Vec<RefundRecord> = self
            .lock_refunds()
            .values()
            .filter(|record| record.provider == provider && record.out_trade_no == out_trade_no)
            .cloned()
            .collect();
        found.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.out_refund_no.cmp(&right.out_refund_no))
        });
        Box::pin(async move { Ok(found) })
    }

    fn transition_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
        next: RefundStatus,
        refund_id: Option<String>,
    ) -> BoxFuture<Result<RefundRecord, PayError>> {
        let result = {
            let mut refunds = self.lock_refunds();
            let key = (provider.to_owned(), out_refund_no.to_owned());
            match refunds.get_mut(&key) {
                Some(record) => record.status.transition(next).map(|status| {
                    record.status = status;
                    if refund_id.is_some() {
                        record.refund_id = refund_id;
                    }
                    record.clone()
                }),
                None => Err(PayError::RefundNotFound {
                    provider: provider.to_owned(),
                    out_refund_no: out_refund_no.to_owned(),
                }),
            }
        };
        Box::pin(async move { result })
    }

    fn record_refund_id(
        &self,
        provider: &str,
        out_refund_no: &str,
        refund_id: &str,
    ) -> BoxFuture<Result<(), PayError>> {
        let result = {
            let mut refunds = self.lock_refunds();
            match refunds.get_mut(&(provider.to_owned(), out_refund_no.to_owned())) {
                Some(record) => {
                    record.refund_id = Some(refund_id.to_owned());
                    Ok(())
                }
                None => Err(PayError::RefundNotFound {
                    provider: provider.to_owned(),
                    out_refund_no: out_refund_no.to_owned(),
                }),
            }
        };
        Box::pin(async move { result })
    }
}
