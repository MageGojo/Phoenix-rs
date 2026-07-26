use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::{
    Amount, Bill, CreateOrder, MemoryPaymentStore, NotifyEvent, NotifyRequest, PayError,
    PaymentIntent, PaymentProvider, PaymentRecord, PaymentStatus, PaymentStore, Reconciliation,
    RefundOrder, RefundReceipt, RefundRecord, RefundStatus, reconcile,
};

/// Outcome of processing one asynchronous notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NotifyOutcome {
    /// First delivery: the order transitioned and the event was recorded.
    Processed(NotifyEvent),
    /// Idempotent replay: the order is already in the reported status.
    AlreadyProcessed(NotifyEvent),
}

/// Facade tying providers to the [`PaymentStore`].
///
/// Notification handling is idempotent on `(provider, out_trade_no)`: the
/// first verified notify transitions the order, replays return
/// [`NotifyOutcome::AlreadyProcessed`] without touching state.
pub struct PayManager {
    providers: BTreeMap<&'static str, Arc<dyn PaymentProvider>>,
    store: Arc<dyn PaymentStore>,
}

/// Builder for [`PayManager`].
pub struct PayManagerBuilder {
    providers: BTreeMap<&'static str, Arc<dyn PaymentProvider>>,
    store: Arc<dyn PaymentStore>,
}

impl Default for PayManagerBuilder {
    fn default() -> Self {
        Self {
            providers: BTreeMap::new(),
            store: Arc::new(MemoryPaymentStore::new()),
        }
    }
}

impl PayManagerBuilder {
    /// Register one provider under its [`PaymentProvider::key`].
    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn PaymentProvider>) -> Self {
        self.providers.insert(provider.key(), provider);
        self
    }

    /// Replace the default [`MemoryPaymentStore`].
    #[must_use]
    pub fn store(mut self, store: Arc<dyn PaymentStore>) -> Self {
        self.store = store;
        self
    }

    /// Finish building.
    #[must_use]
    pub fn build(self) -> PayManager {
        PayManager {
            providers: self.providers,
            store: self.store,
        }
    }
}

impl PayManager {
    /// Start a builder with an in-memory store and no providers.
    #[must_use]
    pub fn builder() -> PayManagerBuilder {
        PayManagerBuilder::default()
    }

    /// Registered provider keys in sorted order.
    #[must_use]
    pub fn providers(&self) -> Vec<&'static str> {
        self.providers.keys().copied().collect()
    }

    /// The bound store.
    #[must_use]
    pub fn store(&self) -> Arc<dyn PaymentStore> {
        Arc::clone(&self.store)
    }

    fn provider(&self, key: &str) -> Result<&Arc<dyn PaymentProvider>, PayError> {
        self.providers
            .get(key)
            .ok_or_else(|| PayError::UnknownProvider(key.to_owned()))
    }

    /// Create an order: validate, persist as `Created`, call the provider,
    /// then move to `Pending`.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers, invalid or duplicate
    /// orders, and provider failures (the record is marked `Failed` on a
    /// provider error, best effort).
    pub async fn create(
        &self,
        provider_key: &str,
        order: CreateOrder,
    ) -> Result<PaymentIntent, PayError> {
        let provider = self.provider(provider_key)?;
        order.validate()?;
        self.store
            .insert(PaymentRecord::new(
                provider.key(),
                &order,
                SystemTime::now(),
            ))
            .await?;
        match provider.create(&order).await {
            Ok(intent) => {
                self.store
                    .transition(
                        provider.key(),
                        &order.out_trade_no,
                        PaymentStatus::Pending,
                        None,
                    )
                    .await?;
                Ok(intent)
            }
            Err(error) => {
                // Best effort: park the record so the out_trade_no is not stuck
                // in `Created` (Created -> Closed keeps the row auditable).
                let _ = self
                    .store
                    .transition(
                        provider.key(),
                        &order.out_trade_no,
                        PaymentStatus::Closed,
                        None,
                    )
                    .await;
                Err(error)
            }
        }
    }

    /// Verify and apply one asynchronous notification, idempotently.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers, verification failures,
    /// unknown orders, and transitions the state machine rejects (other than
    /// idempotent replays, which succeed with
    /// [`NotifyOutcome::AlreadyProcessed`]).
    pub async fn handle_notify(
        &self,
        provider_key: &str,
        notify: NotifyRequest,
    ) -> Result<NotifyOutcome, PayError> {
        let provider = self.provider(provider_key)?;
        let event = provider.verify_notify(&notify).await?;
        let record = self
            .store
            .find(provider.key(), &event.out_trade_no)
            .await?
            .ok_or_else(|| PayError::OrderNotFound {
                provider: provider.key().to_owned(),
                out_trade_no: event.out_trade_no.clone(),
            })?;
        if record.status == event.status {
            return Ok(NotifyOutcome::AlreadyProcessed(event));
        }
        self.store
            .transition(
                provider.key(),
                &event.out_trade_no,
                event.status,
                Some(event.raw.clone()),
            )
            .await?;
        Ok(NotifyOutcome::Processed(event))
    }

    /// Query the provider-side status of an order.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers or provider failures.
    pub async fn query(
        &self,
        provider_key: &str,
        out_trade_no: &str,
    ) -> Result<PaymentStatus, PayError> {
        self.provider(provider_key)?.query(out_trade_no).await
    }

    /// Close an unpaid order on the provider side, then move the stored
    /// record to [`PaymentStatus::Closed`].
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers, provider failures
    /// (including [`PayError::NotImplemented`] for providers without a close
    /// API), and transitions the state machine rejects (e.g. the order is
    /// already `Paid`).
    pub async fn close(&self, provider_key: &str, out_trade_no: &str) -> Result<(), PayError> {
        let provider = self.provider(provider_key)?;
        provider.close(out_trade_no).await?;
        self.store
            .transition(provider.key(), out_trade_no, PaymentStatus::Closed, None)
            .await?;
        Ok(())
    }

    /// Fetch the locally stored record for an order.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::Store`] when the store fails.
    pub async fn find_order(
        &self,
        provider_key: &str,
        out_trade_no: &str,
    ) -> Result<Option<PaymentRecord>, PayError> {
        self.store.find(provider_key, out_trade_no).await
    }

    /// Refund a paid order, in full or in part.
    ///
    /// The sequence is deliberate — the local record is written **before** the
    /// provider is called, so a refund that succeeds at the gateway but crashes
    /// the process on the way back is still recoverable from the row rather
    /// than lost:
    ///
    /// 1. validate, and re-request idempotently if `out_refund_no` is known;
    /// 2. check the order is `Paid`/`Refunding` and that the refundable
    ///    remainder covers this request;
    /// 3. persist the refund as `Processing`;
    /// 4. call the provider;
    /// 5. record the outcome and move the order's own status.
    ///
    /// A provider error marks the stored refund `Failed`, which releases its
    /// amount again, so a retry under a new number is not blocked.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers, invalid refunds, orders that
    /// are not refundable, over-refunds, provider failures (including
    /// [`PayError::NotImplemented`] for providers without a refund API), and
    /// store failures.
    pub async fn refund(
        &self,
        provider_key: &str,
        refund: RefundOrder,
    ) -> Result<RefundReceipt, PayError> {
        let provider = self.provider(provider_key)?;
        refund.validate()?;

        // Idempotent re-request: a known refund number never charges twice.
        if let Some(existing) = self
            .store
            .find_refund(provider.key(), &refund.out_refund_no)
            .await?
        {
            if existing.out_trade_no != refund.out_trade_no {
                return Err(PayError::InvalidRefund(
                    "out_refund_no is already used by a different order",
                ));
            }
            return Ok(RefundReceipt {
                provider: provider.key().to_owned(),
                out_trade_no: existing.out_trade_no,
                out_refund_no: existing.out_refund_no,
                refund_id: existing.refund_id,
                amount: existing.amount,
                status: existing.status,
                raw: String::new(),
            });
        }

        let record = self
            .store
            .find(provider.key(), &refund.out_trade_no)
            .await?
            .ok_or_else(|| PayError::OrderNotFound {
                provider: provider.key().to_owned(),
                out_trade_no: refund.out_trade_no.clone(),
            })?;
        if !matches!(
            record.status,
            PaymentStatus::Paid | PaymentStatus::Refunding
        ) {
            return Err(PayError::InvalidRefund("only a paid order can be refunded"));
        }
        if record.amount != refund.total {
            return Err(PayError::InvalidRefund(
                "refund total does not match the stored order amount",
            ));
        }
        let refundable = self.refundable(provider.key(), &record).await?;
        if refund.amount.minor() > refundable.minor() {
            return Err(PayError::RefundExceedsOrder {
                out_trade_no: refund.out_trade_no.clone(),
                requested: refund.amount,
                refundable,
            });
        }

        self.store
            .insert_refund(RefundRecord::new(
                provider.key(),
                &refund,
                SystemTime::now(),
            ))
            .await?;

        let receipt = match provider.refund(&refund).await {
            Ok(receipt) => receipt,
            Err(error) => {
                // Release the reserved amount so a retry is not blocked by a
                // refund that never reached the provider.
                let _ = self
                    .store
                    .transition_refund(
                        provider.key(),
                        &refund.out_refund_no,
                        RefundStatus::Failed,
                        None,
                    )
                    .await;
                return Err(error);
            }
        };

        if receipt.status != RefundStatus::Processing {
            self.store
                .transition_refund(
                    provider.key(),
                    &refund.out_refund_no,
                    receipt.status,
                    receipt.refund_id.clone(),
                )
                .await?;
        } else if let Some(refund_id) = &receipt.refund_id {
            // Still processing, but record the provider id for later polling.
            self.store
                .record_refund_id(provider.key(), &refund.out_refund_no, refund_id)
                .await?;
        }

        self.sync_order_refund_status(provider.key(), &refund.out_trade_no)
            .await?;
        Ok(receipt)
    }

    /// Poll one refund at the provider and apply the outcome locally.
    ///
    /// Use this for refunds that came back [`RefundStatus::Processing`]. It is
    /// safe to call repeatedly: a refund that already settled is returned
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers or refunds, provider
    /// failures, and store failures.
    pub async fn sync_refund(
        &self,
        provider_key: &str,
        out_refund_no: &str,
    ) -> Result<RefundRecord, PayError> {
        let provider = self.provider(provider_key)?;
        let stored = self
            .store
            .find_refund(provider.key(), out_refund_no)
            .await?
            .ok_or_else(|| PayError::RefundNotFound {
                provider: provider.key().to_owned(),
                out_refund_no: out_refund_no.to_owned(),
            })?;
        if stored.status.is_terminal() {
            return Ok(stored);
        }

        let receipt = provider
            .query_refund(&stored.out_trade_no, out_refund_no)
            .await?;
        if receipt.status == RefundStatus::Processing {
            return Ok(stored);
        }
        let updated = self
            .store
            .transition_refund(
                provider.key(),
                out_refund_no,
                receipt.status,
                receipt.refund_id,
            )
            .await?;
        self.sync_order_refund_status(provider.key(), &stored.out_trade_no)
            .await?;
        Ok(updated)
    }

    /// Every refund recorded against one order, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::Store`] when the store fails.
    pub async fn refunds_for(
        &self,
        provider_key: &str,
        out_trade_no: &str,
    ) -> Result<Vec<RefundRecord>, PayError> {
        self.store.refunds_for(provider_key, out_trade_no).await
    }

    /// How much of `record` may still be refunded: the order total minus every
    /// refund that has not failed.
    async fn refundable(
        &self,
        provider_key: &str,
        record: &PaymentRecord,
    ) -> Result<Amount, PayError> {
        let held: u64 = self
            .store
            .refunds_for(provider_key, &record.out_trade_no)
            .await?
            .iter()
            .filter(|refund| refund.holds_amount())
            .map(|refund| refund.amount.minor())
            .sum();
        Ok(Amount::from_minor(
            record.amount.minor().saturating_sub(held),
            record.amount.currency(),
        ))
    }

    /// Move the order's own status to match its refunds: `Paid -> Refunding`
    /// once one is in flight, `Refunding -> Refunded` once the succeeded total
    /// covers the order, and `Refunding -> Paid` when every refund failed.
    async fn sync_order_refund_status(
        &self,
        provider_key: &str,
        out_trade_no: &str,
    ) -> Result<(), PayError> {
        let record = self
            .store
            .find(provider_key, out_trade_no)
            .await?
            .ok_or_else(|| PayError::OrderNotFound {
                provider: provider_key.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            })?;
        let refunds = self.store.refunds_for(provider_key, out_trade_no).await?;
        let succeeded: u64 = refunds
            .iter()
            .filter(|refund| refund.status == RefundStatus::Succeeded)
            .map(|refund| refund.amount.minor())
            .sum();
        let outstanding = refunds
            .iter()
            .any(|refund| refund.status == RefundStatus::Processing);

        let target = if succeeded >= record.amount.minor() {
            PaymentStatus::Refunded
        } else if succeeded > 0 || outstanding {
            PaymentStatus::Refunding
        } else {
            PaymentStatus::Paid
        };
        if target == record.status {
            return Ok(());
        }
        // Paid -> Refunded is not a legal single step; go through Refunding.
        if record.status == PaymentStatus::Paid && target == PaymentStatus::Refunded {
            self.store
                .transition(provider_key, out_trade_no, PaymentStatus::Refunding, None)
                .await?;
        }
        self.store
            .transition(provider_key, out_trade_no, target, None)
            .await?;
        Ok(())
    }

    /// Download a provider's bill for `date` (`YYYY-MM-DD`) and compare it
    /// against the local orders paid that day.
    ///
    /// `day_start` is the instant the provider's billing day begins, in real
    /// time — the caller supplies it because the gateways bill in their own
    /// timezone (both CN gateways use UTC+8) and this crate deliberately owns
    /// no timezone database. The window compared is `[day_start, day_start + 24h)`.
    ///
    /// # Errors
    ///
    /// Returns [`PayError`] for unknown providers, provider or parse failures
    /// (including [`PayError::NotImplemented`] for providers without a bill
    /// API), and store failures.
    pub async fn reconcile_day(
        &self,
        provider_key: &str,
        date: &str,
        day_start: SystemTime,
    ) -> Result<Reconciliation, PayError> {
        let provider = self.provider(provider_key)?;
        let bill = provider.download_bill(date).await?;
        self.reconcile_bill(&bill, day_start).await
    }

    /// Compare an already-downloaded bill against the local orders paid in the
    /// 24 hours starting at `day_start`.
    ///
    /// Use this when the bill came from somewhere else — a file the finance
    /// team dropped, or a provider whose bill format this crate does not parse.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::Store`] when the store fails.
    pub async fn reconcile_bill(
        &self,
        bill: &Bill,
        day_start: SystemTime,
    ) -> Result<Reconciliation, PayError> {
        let local = self
            .store
            .paid_within(
                &bill.provider,
                day_start,
                day_start + Duration::from_hours(24),
            )
            .await?;
        Ok(reconcile(bill, &local))
    }
}

impl std::fmt::Debug for PayManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PayManager")
            .field("providers", &self.providers())
            .finish_non_exhaustive()
    }
}
