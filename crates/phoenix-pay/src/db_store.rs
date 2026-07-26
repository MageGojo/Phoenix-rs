//! Toasty-backed [`PaymentStore`] persisting the `payments` table.
//!
//! [`MemoryPaymentStore`](crate::MemoryPaymentStore) keeps orders in process
//! memory; [`DbPaymentStore`] persists them through the Toasty ORM so orders
//! survive a restart. Both implement the same [`PaymentStore`] trait, so a
//! [`PayManager`](crate::PayManager) is built the same way with either.
//!
//! The [`PaymentRow`] model mirrors the columns shipped by the `payments`
//! migration and keeps `(provider, out_trade_no)` unique. Register it in the
//! application `models!(...)` so the shared database knows the table:
//!
//! ```ignore
//! let db = Database::builder(models!(crate::*, phoenix_pay::PaymentRow))
//!     .connect(&url)
//!     .await?;
//! let manager = PayManager::builder()
//!     .provider(Arc::new(provider))
//!     .store(Arc::new(DbPaymentStore::new(db)))
//!     .build();
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_database::Database;
use phoenix_http::BoxFuture;
use toasty::Model;

use crate::{
    Amount, Currency, PayError, PaymentRecord, PaymentStatus, PaymentStore, RefundRecord,
    RefundStatus,
};

/// One row of the `payments` table, as a Toasty model.
///
/// Column mapping matches the `payments` migration shipped by
/// [`PayFeature`](crate::PayFeature): a database-assigned `id`, the money split
/// into `amount` (integer minor units) plus `currency`, `status` as its stable
/// lowercase name, and a `UNIQUE (provider, out_trade_no)` index enforcing the
/// idempotency key.
///
/// Applications register this model in their `models!(...)` set; queries stay
/// inside [`DbPaymentStore`].
#[derive(Debug, Model)]
#[table = "payments"]
#[unique(provider, out_trade_no)]
pub struct PaymentRow {
    /// Database-assigned surrogate key.
    #[key]
    #[auto]
    pub id: i64,
    /// Provider key.
    pub provider: String,
    /// Merchant order number, unique per provider.
    pub out_trade_no: String,
    /// Ordered amount in integer minor units (分 for CNY).
    pub amount: i64,
    /// ISO 4217 currency code.
    pub currency: String,
    /// State-machine status as its stable lowercase name.
    pub status: String,
    /// Order subject.
    pub subject: String,
    /// Last verified notification payload, if any.
    pub notify_payload: Option<String>,
    /// Creation timestamp (nanoseconds since the epoch, zero-padded to 20).
    pub created_at: String,
    /// When the order first reached `paid`; `None` until it does. Indexed
    /// because daily reconciliation queries a window over it.
    #[index]
    pub paid_at: Option<String>,
}

/// One row of the `payment_refunds` table, as a Toasty model.
///
/// `(provider, out_refund_no)` is unique — the same idempotency guarantee the
/// `payments` table gives `(provider, out_trade_no)`.
#[derive(Debug, Model)]
#[table = "payment_refunds"]
#[unique(provider, out_refund_no)]
pub struct RefundRow {
    /// Database-assigned surrogate key.
    #[key]
    #[auto]
    pub id: i64,
    /// Provider key.
    pub provider: String,
    /// Merchant order number this refund belongs to.
    #[index]
    pub out_trade_no: String,
    /// Merchant refund number, unique per provider.
    pub out_refund_no: String,
    /// Provider-side refund id, once known.
    pub refund_id: Option<String>,
    /// Refunded amount in integer minor units.
    pub amount: i64,
    /// ISO 4217 currency code.
    pub currency: String,
    /// Refund status as its stable lowercase name.
    pub status: String,
    /// Optional reason recorded with the request.
    pub reason: Option<String>,
    /// Request timestamp (nanoseconds since the epoch, zero-padded to 20).
    pub created_at: String,
}

/// Toasty-backed [`PaymentStore`].
///
/// Holds a cheaply cloneable [`Database`] handle (an `Arc` over the connection
/// pool); every call borrows a fresh handle, so the store is `Send + Sync` and
/// safe to share behind an `Arc<dyn PaymentStore>`.
#[derive(Clone)]
pub struct DbPaymentStore {
    database: Database,
}

impl DbPaymentStore {
    /// Wrap a [`Database`] whose `models!(...)` set includes [`PaymentRow`].
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

impl std::fmt::Debug for DbPaymentStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DbPaymentStore")
            .field("backend", &self.database.backend())
            .finish()
    }
}

impl PaymentStore for DbPaymentStore {
    fn insert(&self, record: PaymentRecord) -> BoxFuture<Result<(), PayError>> {
        let mut database = self.database.clone();
        Box::pin(async move {
            let existing = PaymentRow::filter_by_provider_and_out_trade_no(
                record.provider.clone(),
                record.out_trade_no.clone(),
            )
            .first()
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))?;
            if existing.is_some() {
                return Err(PayError::DuplicateOrder {
                    provider: record.provider,
                    out_trade_no: record.out_trade_no,
                });
            }

            let amount = i64::try_from(record.amount.minor())
                .map_err(|_| PayError::Store("amount exceeds i64 minor units".to_owned()))?;

            let mut builder = PaymentRow::create()
                .provider(record.provider.clone())
                .out_trade_no(record.out_trade_no.clone())
                .amount(amount)
                .currency(record.amount.currency().code().to_owned())
                .status(record.status.as_str().to_owned())
                .subject(record.subject.clone())
                .created_at(encode_time(record.created_at));
            if let Some(payload) = &record.notify_payload {
                builder = builder.notify_payload(Some(payload.clone()));
            }
            if let Some(paid_at) = record.paid_at {
                builder = builder.paid_at(Some(encode_time(paid_at)));
            }
            builder
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }

    fn find(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Option<PaymentRecord>, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move {
            let row = PaymentRow::filter_by_provider_and_out_trade_no(provider, out_trade_no)
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            row.map(row_into_record).transpose()
        })
    }

    fn transition(
        &self,
        provider: &str,
        out_trade_no: &str,
        next: PaymentStatus,
        notify_payload: Option<String>,
    ) -> BoxFuture<Result<PaymentRecord, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move {
            let Some(mut row) = PaymentRow::filter_by_provider_and_out_trade_no(
                provider.clone(),
                out_trade_no.clone(),
            )
            .first()
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))?
            else {
                return Err(PayError::OrderNotFound {
                    provider,
                    out_trade_no,
                });
            };

            // The state machine rejects illegal moves (and no-op same-status
            // moves); idempotency for replays lives in `PayManager`.
            let current = parse_status(&row.status)?;
            let next_status = current.transition(next)?;

            // Stamped once: a later Refunding -> Paid must not move the order
            // into a different reconciliation day.
            let stamp_paid_at = next_status == PaymentStatus::Paid && row.paid_at.is_none();
            let mut builder = row.update().status(next_status.as_str().to_owned());
            if let Some(payload) = &notify_payload {
                builder = builder.notify_payload(Some(payload.clone()));
            }
            if stamp_paid_at {
                builder = builder.paid_at(Some(encode_time(SystemTime::now())));
            }
            builder
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;

            // `row` is reloaded in place: `status` (and `notify_payload` /
            // `paid_at` when set) now reflect the applied transition.
            row_into_record(row)
        })
    }

    fn paid_within(
        &self,
        provider: &str,
        from: SystemTime,
        to: SystemTime,
    ) -> BoxFuture<Result<Vec<PaymentRecord>, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        Box::pin(async move {
            // `paid_at` is a fixed-width nanosecond string, so a lexicographic
            // range is a chronological range.
            let filter = PaymentRow::fields()
                .provider()
                .eq(provider)
                .and(PaymentRow::fields().paid_at().ge(encode_time(from)))
                .and(PaymentRow::fields().paid_at().lt(encode_time(to)));
            let rows = PaymentRow::all()
                .filter(filter)
                .order_by(PaymentRow::fields().paid_at().asc())
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            rows.into_iter().map(row_into_record).collect()
        })
    }

    fn insert_refund(&self, record: RefundRecord) -> BoxFuture<Result<(), PayError>> {
        let mut database = self.database.clone();
        Box::pin(async move {
            let existing = RefundRow::filter_by_provider_and_out_refund_no(
                record.provider.clone(),
                record.out_refund_no.clone(),
            )
            .first()
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))?;
            if existing.is_some() {
                return Err(PayError::DuplicateRefund {
                    provider: record.provider,
                    out_refund_no: record.out_refund_no,
                });
            }

            let amount = i64::try_from(record.amount.minor())
                .map_err(|_| PayError::Store("amount exceeds i64 minor units".to_owned()))?;
            let mut builder = RefundRow::create()
                .provider(record.provider.clone())
                .out_trade_no(record.out_trade_no.clone())
                .out_refund_no(record.out_refund_no.clone())
                .amount(amount)
                .currency(record.amount.currency().code().to_owned())
                .status(record.status.as_str().to_owned())
                .created_at(encode_time(record.created_at));
            if let Some(refund_id) = &record.refund_id {
                builder = builder.refund_id(Some(refund_id.clone()));
            }
            if let Some(reason) = &record.reason {
                builder = builder.reason(Some(reason.clone()));
            }
            builder
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }

    fn find_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
    ) -> BoxFuture<Result<Option<RefundRecord>, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_refund_no = out_refund_no.to_owned();
        Box::pin(async move {
            let row = RefundRow::filter_by_provider_and_out_refund_no(provider, out_refund_no)
                .first()
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            row.map(row_into_refund).transpose()
        })
    }

    fn refunds_for(
        &self,
        provider: &str,
        out_trade_no: &str,
    ) -> BoxFuture<Result<Vec<RefundRecord>, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_trade_no = out_trade_no.to_owned();
        Box::pin(async move {
            let filter = RefundRow::fields()
                .provider()
                .eq(provider)
                .and(RefundRow::fields().out_trade_no().eq(out_trade_no));
            let rows = RefundRow::all()
                .filter(filter)
                .order_by(RefundRow::fields().created_at().asc())
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            rows.into_iter().map(row_into_refund).collect()
        })
    }

    fn transition_refund(
        &self,
        provider: &str,
        out_refund_no: &str,
        next: RefundStatus,
        refund_id: Option<String>,
    ) -> BoxFuture<Result<RefundRecord, PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_refund_no = out_refund_no.to_owned();
        Box::pin(async move {
            let Some(mut row) = RefundRow::filter_by_provider_and_out_refund_no(
                provider.clone(),
                out_refund_no.clone(),
            )
            .first()
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))?
            else {
                return Err(PayError::RefundNotFound {
                    provider,
                    out_refund_no,
                });
            };

            let current = parse_refund_status(&row.status)?;
            let next_status = current.transition(next)?;
            let mut builder = row.update().status(next_status.as_str().to_owned());
            if let Some(id) = &refund_id {
                builder = builder.refund_id(Some(id.clone()));
            }
            builder
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            row_into_refund(row)
        })
    }

    fn record_refund_id(
        &self,
        provider: &str,
        out_refund_no: &str,
        refund_id: &str,
    ) -> BoxFuture<Result<(), PayError>> {
        let mut database = self.database.clone();
        let provider = provider.to_owned();
        let out_refund_no = out_refund_no.to_owned();
        let refund_id = refund_id.to_owned();
        Box::pin(async move {
            let Some(mut row) = RefundRow::filter_by_provider_and_out_refund_no(
                provider.clone(),
                out_refund_no.clone(),
            )
            .first()
            .exec(database.toasty_mut())
            .await
            .map_err(|error| to_store_error(&error))?
            else {
                return Err(PayError::RefundNotFound {
                    provider,
                    out_refund_no,
                });
            };
            row.update()
                .refund_id(Some(refund_id))
                .exec(database.toasty_mut())
                .await
                .map_err(|error| to_store_error(&error))?;
            Ok(())
        })
    }
}

fn row_into_record(row: PaymentRow) -> Result<PaymentRecord, PayError> {
    let minor = u64::try_from(row.amount)
        .map_err(|_| PayError::Store("stored amount is negative".to_owned()))?;
    Ok(PaymentRecord {
        provider: row.provider,
        out_trade_no: row.out_trade_no,
        amount: Amount::from_minor(minor, parse_currency(&row.currency)?),
        subject: row.subject,
        status: parse_status(&row.status)?,
        notify_payload: row.notify_payload,
        created_at: decode_time(&row.created_at)?,
        paid_at: row.paid_at.as_deref().map(decode_time).transpose()?,
    })
}

fn row_into_refund(row: RefundRow) -> Result<RefundRecord, PayError> {
    let minor = u64::try_from(row.amount)
        .map_err(|_| PayError::Store("stored refund amount is negative".to_owned()))?;
    Ok(RefundRecord {
        provider: row.provider,
        out_trade_no: row.out_trade_no,
        out_refund_no: row.out_refund_no,
        refund_id: row.refund_id,
        amount: Amount::from_minor(minor, parse_currency(&row.currency)?),
        status: parse_refund_status(&row.status)?,
        reason: row.reason,
        created_at: decode_time(&row.created_at)?,
    })
}

fn parse_refund_status(text: &str) -> Result<RefundStatus, PayError> {
    text.parse::<RefundStatus>()
        .map_err(|_| PayError::Store(format!("invalid stored refund status `{text}`")))
}

/// Encode a [`SystemTime`] as a fixed-width, lexicographically sortable
/// nanoseconds-since-epoch string, so a `TEXT` range query over `paid_at` is a
/// chronological range query.
fn encode_time(time: SystemTime) -> String {
    let nanos = time
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or_default();
    let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
    format!("{nanos:020}")
}

fn decode_time(text: &str) -> Result<SystemTime, PayError> {
    let nanos: u64 = text
        .trim()
        .parse()
        .map_err(|_| PayError::Store(format!("invalid stored timestamp `{text}`")))?;
    Ok(UNIX_EPOCH + Duration::from_nanos(nanos))
}

fn parse_status(text: &str) -> Result<PaymentStatus, PayError> {
    text.parse::<PaymentStatus>()
        .map_err(|_| PayError::Store(format!("invalid stored status `{text}`")))
}

fn parse_currency(code: &str) -> Result<Currency, PayError> {
    match code {
        "CNY" => Ok(Currency::Cny),
        other => Err(PayError::Store(format!(
            "unsupported stored currency `{other}`"
        ))),
    }
}

fn to_store_error(error: &toasty::Error) -> PayError {
    PayError::Store(error.to_string())
}
