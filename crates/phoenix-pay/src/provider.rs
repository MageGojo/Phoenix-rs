use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use phoenix_http::BoxFuture;
use serde::Deserialize;

use crate::{
    Amount, Bill, BillEntry, CreateOrder, NotifyEvent, NotifyRequest, PayError, PaymentAction,
    PaymentIntent, PaymentStatus, RefundOrder, RefundReceipt, RefundStatus,
};

/// A payment channel implementation (`WeChat` Native, Alipay F2F, mock, ...).
///
/// Async style matches [`phoenix_mail::MailTransport`]: object-safe methods
/// returning [`phoenix_http::BoxFuture`], so managers can hold
/// `Arc<dyn PaymentProvider>`.
///
/// [`phoenix_mail::MailTransport`]: https://docs.rs/phoenix-mail
pub trait PaymentProvider: Send + Sync {
    /// Stable provider key (`"mock"`, `"wechat_native"`, `"alipay_f2f"`).
    /// Used for routing notifications and as part of the idempotency key.
    fn key(&self) -> &'static str;

    /// Create an order on the provider side and describe the next payer step.
    fn create(&self, order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>>;

    /// Verify an asynchronous notification and normalize it.
    ///
    /// Implementations MUST authenticate the payload (signature / decryption)
    /// before trusting any field; returning an unverified event is a bug.
    fn verify_notify(&self, notify: &NotifyRequest) -> BoxFuture<Result<NotifyEvent, PayError>>;

    /// Query the provider-side status of an order.
    fn query(&self, out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>>;

    /// Close an unpaid order on the provider side (`WeChat` close /
    /// `alipay.trade.close`). Providers without a close API keep the default
    /// [`PayError::NotImplemented`].
    fn close(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        let _ = out_trade_no;
        Box::pin(async {
            Err(PayError::NotImplemented(
                "close not supported by this provider",
            ))
        })
    }

    /// Refund a paid order, in full or in part.
    ///
    /// `refund.out_refund_no` is the idempotency key: re-sending the same
    /// number for the same order must not move money twice, and both real
    /// gateways enforce that server-side. A provider may answer
    /// [`RefundStatus::Processing`](crate::RefundStatus) — that is success,
    /// not failure; poll [`Self::query_refund`] for the outcome.
    fn refund(&self, refund: &RefundOrder) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let _ = refund;
        Box::pin(async {
            Err(PayError::NotImplemented(
                "refund not supported by this provider",
            ))
        })
    }

    /// Query one refund's provider-side state.
    fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let _ = (out_trade_no, out_refund_no);
        Box::pin(async {
            Err(PayError::NotImplemented(
                "refund query not supported by this provider",
            ))
        })
    }

    /// Download the provider's settled trade bill for one day (`YYYY-MM-DD`).
    ///
    /// The bill is the provider's own record of what moved, which is what makes
    /// it worth reconciling against. Feed the result to
    /// [`reconcile`](crate::reconcile).
    fn download_bill(&self, date: &str) -> BoxFuture<Result<Bill, PayError>> {
        let _ = date;
        Box::pin(async {
            Err(PayError::NotImplemented(
                "bill download not supported by this provider",
            ))
        })
    }
}

/// Notification body format understood by [`MockProvider::verify_notify`].
#[derive(Deserialize)]
struct MockNotifyBody {
    out_trade_no: String,
    status: PaymentStatus,
    #[serde(default)]
    transaction_id: Option<String>,
}

/// Fully working in-process provider for tests and local development.
///
/// `create` returns a deterministic QR-code text; tests then call
/// [`MockProvider::paid_notify_body`] and feed it through the webhook (or
/// [`crate::PayManager::handle_notify`]) to drive the order to `Paid`.
#[derive(Clone, Default)]
pub struct MockProvider {
    orders: Arc<Mutex<HashMap<String, MockOrder>>>,
    /// Refund numbers the provider should answer `Processing` for, so tests can
    /// drive the asynchronous branch (`WeChat` bank refunds behave this way).
    slow_refunds: Arc<Mutex<HashMap<String, bool>>>,
}

/// Provider-side view of one mock order.
#[derive(Clone, Debug)]
struct MockOrder {
    status: PaymentStatus,
    amount: Amount,
    /// Accepted refunds by `out_refund_no`, in request order.
    refunds: Vec<(String, Amount, RefundStatus)>,
}

impl MockProvider {
    /// Provider key registered by [`Self::key`].
    pub const KEY: &'static str = "mock";

    /// Fresh provider with no orders.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulate the payer scanning and paying: marks the provider-side order
    /// as paid and returns the notification body to POST to the webhook.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::OrderNotFound`] when `create` was never called for
    /// `out_trade_no`.
    pub fn mark_paid(&self, out_trade_no: &str) -> Result<String, PayError> {
        let mut orders = self.lock();
        match orders.get_mut(out_trade_no) {
            Some(order) => {
                order.status = PaymentStatus::Paid;
                Ok(Self::paid_notify_body(out_trade_no))
            }
            None => Err(PayError::OrderNotFound {
                provider: Self::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            }),
        }
    }

    /// Make [`Self::refund`] answer [`RefundStatus::Processing`] for
    /// `out_refund_no` instead of settling immediately, so tests can exercise
    /// the asynchronous branch. Call [`Self::settle_refund`] to finish it.
    pub fn defer_refund(&self, out_refund_no: &str) {
        self.slow_refunds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(out_refund_no.to_owned(), true);
    }

    /// Settle a deferred refund, so the next [`Self::query_refund`] reports the
    /// given outcome.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::RefundNotFound`] when no such refund was requested.
    pub fn settle_refund(&self, out_refund_no: &str, status: RefundStatus) -> Result<(), PayError> {
        let mut orders = self.lock();
        for order in orders.values_mut() {
            if let Some(refund) = order
                .refunds
                .iter_mut()
                .find(|(number, _, _)| number == out_refund_no)
            {
                refund.2 = status;
                return Ok(());
            }
        }
        Err(PayError::RefundNotFound {
            provider: Self::KEY.to_owned(),
            out_refund_no: out_refund_no.to_owned(),
        })
    }

    /// The JSON notification body [`Self::verify_notify`] accepts for a paid order.
    #[must_use]
    pub fn paid_notify_body(out_trade_no: &str) -> String {
        serde_json::json!({
            "out_trade_no": out_trade_no,
            "status": "paid",
            "transaction_id": format!("MOCK-{out_trade_no}"),
        })
        .to_string()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, MockOrder>> {
        self.orders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn is_deferred(&self, out_refund_no: &str) -> bool {
        self.slow_refunds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(out_refund_no)
    }
}

impl std::fmt::Debug for MockProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MockProvider")
            .field("orders", &self.lock().len())
            .finish()
    }
}

impl PaymentProvider for MockProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn create(&self, order: &CreateOrder) -> BoxFuture<Result<PaymentIntent, PayError>> {
        let result = order.validate().map(|()| {
            self.lock().insert(
                order.out_trade_no.clone(),
                MockOrder {
                    status: PaymentStatus::Pending,
                    amount: order.amount,
                    refunds: Vec::new(),
                },
            );
            PaymentIntent {
                provider: Self::KEY.to_owned(),
                out_trade_no: order.out_trade_no.clone(),
                amount: order.amount,
                action: PaymentAction::QrCode(format!(
                    "mockpay://qr/{}?amount={}",
                    order.out_trade_no,
                    order.amount.minor()
                )),
            }
        });
        Box::pin(async move { result })
    }

    fn verify_notify(&self, notify: &NotifyRequest) -> BoxFuture<Result<NotifyEvent, PayError>> {
        let result = notify.body_str().and_then(|raw| {
            let parsed: MockNotifyBody = serde_json::from_str(raw)
                .map_err(|error| PayError::InvalidNotify(error.to_string()))?;
            Ok(NotifyEvent {
                out_trade_no: parsed.out_trade_no,
                transaction_id: parsed.transaction_id,
                status: parsed.status,
                raw: raw.to_owned(),
            })
        });
        Box::pin(async move { result })
    }

    fn query(&self, out_trade_no: &str) -> BoxFuture<Result<PaymentStatus, PayError>> {
        let result = self
            .lock()
            .get(out_trade_no)
            .map(|order| order.status)
            .ok_or_else(|| PayError::OrderNotFound {
                provider: Self::KEY.to_owned(),
                out_trade_no: out_trade_no.to_owned(),
            });
        Box::pin(async move { result })
    }

    fn close(&self, out_trade_no: &str) -> BoxFuture<Result<(), PayError>> {
        let result = {
            let mut orders = self.lock();
            match orders.get_mut(out_trade_no) {
                Some(order) => {
                    order.status = PaymentStatus::Closed;
                    Ok(())
                }
                None => Err(PayError::OrderNotFound {
                    provider: Self::KEY.to_owned(),
                    out_trade_no: out_trade_no.to_owned(),
                }),
            }
        };
        Box::pin(async move { result })
    }

    fn refund(&self, refund: &RefundOrder) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let deferred = self.is_deferred(&refund.out_refund_no);
        let result = refund.validate().and_then(|()| {
            let mut orders = self.lock();
            let order =
                orders
                    .get_mut(&refund.out_trade_no)
                    .ok_or_else(|| PayError::OrderNotFound {
                        provider: Self::KEY.to_owned(),
                        out_trade_no: refund.out_trade_no.clone(),
                    })?;
            if order.status != PaymentStatus::Paid {
                return Err(PayError::InvalidRefund("only a paid order can be refunded"));
            }
            // Server-side idempotency: the same refund number returns the same
            // receipt instead of moving money twice.
            if let Some((_, amount, status)) = order
                .refunds
                .iter()
                .find(|(number, _, _)| *number == refund.out_refund_no)
                .cloned()
            {
                return Ok(receipt(refund, amount, status));
            }
            let already: u64 = order
                .refunds
                .iter()
                .filter(|(_, _, status)| *status != RefundStatus::Failed)
                .map(|(_, amount, _)| amount.minor())
                .sum();
            if already + refund.amount.minor() > order.amount.minor() {
                return Err(PayError::RefundExceedsOrder {
                    out_trade_no: refund.out_trade_no.clone(),
                    requested: refund.amount,
                    refundable: Amount::from_minor(
                        order.amount.minor() - already,
                        order.amount.currency(),
                    ),
                });
            }
            let status = if deferred {
                RefundStatus::Processing
            } else {
                RefundStatus::Succeeded
            };
            order
                .refunds
                .push((refund.out_refund_no.clone(), refund.amount, status));
            Ok(receipt(refund, refund.amount, status))
        });
        Box::pin(async move { result })
    }

    fn query_refund(
        &self,
        out_trade_no: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<RefundReceipt, PayError>> {
        let result = {
            let orders = self.lock();
            orders
                .get(out_trade_no)
                .and_then(|order| {
                    order
                        .refunds
                        .iter()
                        .find(|(number, _, _)| number == out_refund_no)
                        .cloned()
                })
                .map(|(number, amount, status)| RefundReceipt {
                    provider: Self::KEY.to_owned(),
                    out_trade_no: out_trade_no.to_owned(),
                    out_refund_no: number,
                    refund_id: Some(format!("MOCK-REFUND-{out_refund_no}")),
                    amount,
                    status,
                    raw: String::new(),
                })
                .ok_or_else(|| PayError::RefundNotFound {
                    provider: Self::KEY.to_owned(),
                    out_refund_no: out_refund_no.to_owned(),
                })
        };
        Box::pin(async move { result })
    }

    fn download_bill(&self, date: &str) -> BoxFuture<Result<Bill, PayError>> {
        // The mock has no calendar, so every known order lands on the requested
        // day; that is enough to exercise the reconciliation path end to end.
        let entries = self
            .lock()
            .iter()
            .map(|(out_trade_no, order)| BillEntry {
                out_trade_no: out_trade_no.clone(),
                transaction_id: Some(format!("MOCK-{out_trade_no}")),
                amount: order.amount,
                refunded: Amount::from_minor(
                    order
                        .refunds
                        .iter()
                        .filter(|(_, _, status)| *status == RefundStatus::Succeeded)
                        .map(|(_, amount, _)| amount.minor())
                        .sum(),
                    order.amount.currency(),
                ),
                status: order.status,
            })
            .collect::<Vec<_>>();
        let mut entries = entries;
        entries.sort_by(|left, right| left.out_trade_no.cmp(&right.out_trade_no));
        let bill = Bill {
            provider: Self::KEY.to_owned(),
            date: date.to_owned(),
            entries,
        };
        Box::pin(async move { Ok(bill) })
    }
}

fn receipt(refund: &RefundOrder, amount: Amount, status: RefundStatus) -> RefundReceipt {
    RefundReceipt {
        provider: MockProvider::KEY.to_owned(),
        out_trade_no: refund.out_trade_no.clone(),
        out_refund_no: refund.out_refund_no.clone(),
        refund_id: Some(format!("MOCK-REFUND-{}", refund.out_refund_no)),
        amount,
        status,
        raw: String::new(),
    }
}
