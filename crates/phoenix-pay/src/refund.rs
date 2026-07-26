//! Refund request, lifecycle, and persisted record.
//!
//! A refund is its own object with its own lifecycle, not a status on the
//! order: one paid order can carry several partial refunds, each with its own
//! merchant number, amount, and outcome. `(provider, out_refund_no)` is the
//! idempotency key, mirroring `(provider, out_trade_no)` for orders.
//!
//! The order's [`PaymentStatus`](crate::PaymentStatus) still moves — `Paid ->
//! Refunding` when the first refund is accepted, `Refunding -> Refunded` once
//! the refunded total reaches the order total, and `Refunding -> Paid` when
//! every outstanding refund failed.

use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Amount, PayError};

/// A merchant-side refund request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefundOrder {
    /// Merchant order number of the paid order being refunded.
    pub out_trade_no: String,
    /// Merchant refund number, unique per provider — the idempotency key.
    pub out_refund_no: String,
    /// Amount to refund; may be less than `total` for a partial refund.
    pub amount: Amount,
    /// Total of the original order. Both real gateways require it, and it is
    /// what makes a partial refund unambiguous.
    pub total: Amount,
    /// Optional reason, shown in the provider console.
    pub reason: Option<String>,
}

impl RefundOrder {
    /// Full refund of `total` under `out_refund_no`.
    #[must_use]
    pub fn full(
        out_trade_no: impl Into<String>,
        out_refund_no: impl Into<String>,
        total: Amount,
    ) -> Self {
        Self {
            out_trade_no: out_trade_no.into(),
            out_refund_no: out_refund_no.into(),
            amount: total,
            total,
            reason: None,
        }
    }

    /// Partial refund of `amount` out of `total`.
    #[must_use]
    pub fn partial(
        out_trade_no: impl Into<String>,
        out_refund_no: impl Into<String>,
        amount: Amount,
        total: Amount,
    ) -> Self {
        Self {
            out_trade_no: out_trade_no.into(),
            out_refund_no: out_refund_no.into(),
            amount,
            total,
            reason: None,
        }
    }

    /// Attach a reason (builder-style).
    #[must_use]
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Validate invariants shared by every provider.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidRefund`] for empty numbers, a zero amount, a
    /// refund larger than the order total, or mixed currencies.
    pub fn validate(&self) -> Result<(), PayError> {
        if self.out_trade_no.trim().is_empty() {
            return Err(PayError::InvalidRefund("out_trade_no must not be empty"));
        }
        if self.out_refund_no.trim().is_empty() {
            return Err(PayError::InvalidRefund("out_refund_no must not be empty"));
        }
        if self.amount.is_zero() {
            return Err(PayError::InvalidRefund(
                "refund amount must be greater than zero",
            ));
        }
        if self.amount.currency() != self.total.currency() {
            return Err(PayError::CurrencyMismatch {
                left: self.amount.currency().code(),
                right: self.total.currency().code(),
            });
        }
        if self.amount.minor() > self.total.minor() {
            return Err(PayError::InvalidRefund(
                "refund amount must not exceed the order total",
            ));
        }
        Ok(())
    }
}

/// Refund lifecycle, independent of the order lifecycle.
///
/// ```text
/// Processing ──> Succeeded
///     │
///     └───────> Failed
/// ```
///
/// Providers may answer `Succeeded` synchronously (Alipay F2F usually does) or
/// `Processing` with the outcome arriving later (`WeChat` bank refunds), so
/// both are normal results of [`crate::PayManager::refund`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundStatus {
    /// Accepted by the provider; the money has not settled yet.
    #[default]
    Processing,
    /// The provider confirmed the money went back to the payer.
    Succeeded,
    /// The provider gave up; the order keeps its paid amount.
    Failed,
}

impl RefundStatus {
    /// Stable lowercase name, used in the `payment_refunds.status` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Processing => "processing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Whether no further change can leave this status.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }

    /// Whether the refund state machine allows `self -> next`.
    ///
    /// Only `Processing` may move, and only to a terminal outcome: a settled
    /// refund never reopens, and a failed one needs a new `out_refund_no`.
    #[must_use]
    pub const fn can_transition(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Processing, Self::Succeeded | Self::Failed)
        )
    }

    /// Validate and perform a transition.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidRefundTransition`] when the move is not
    /// allowed (including no-op transitions to the same status).
    pub fn transition(self, next: Self) -> Result<Self, PayError> {
        if self.can_transition(next) {
            Ok(next)
        } else {
            Err(PayError::InvalidRefundTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for RefundStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RefundStatus {
    type Err = PayError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "processing" => Ok(Self::Processing),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(PayError::InvalidNotify(format!(
                "unknown refund status `{value}`"
            ))),
        }
    }
}

/// A provider's answer to a refund request or refund query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefundReceipt {
    /// Provider key that produced this receipt.
    pub provider: String,
    /// Merchant order number.
    pub out_trade_no: String,
    /// Merchant refund number.
    pub out_refund_no: String,
    /// Provider-side refund id, when the provider assigns one.
    pub refund_id: Option<String>,
    /// Refunded amount as the provider reports it.
    pub amount: Amount,
    /// Refund status as the provider reports it.
    pub status: RefundStatus,
    /// Raw provider payload, kept for auditing.
    pub raw: String,
}

/// One persisted row of the `payment_refunds` table (or its in-memory stand-in).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefundRecord {
    /// Provider key.
    pub provider: String,
    /// Merchant order number this refund belongs to.
    pub out_trade_no: String,
    /// Merchant refund number, unique per provider.
    pub out_refund_no: String,
    /// Provider-side refund id, once known.
    pub refund_id: Option<String>,
    /// Refunded amount.
    pub amount: Amount,
    /// Current refund status.
    pub status: RefundStatus,
    /// Optional reason recorded with the request.
    pub reason: Option<String>,
    /// When the refund was requested.
    pub created_at: SystemTime,
}

impl RefundRecord {
    /// Fresh [`RefundStatus::Processing`] record for a validated request.
    #[must_use]
    pub fn new(provider: impl Into<String>, refund: &RefundOrder, created_at: SystemTime) -> Self {
        Self {
            provider: provider.into(),
            out_trade_no: refund.out_trade_no.clone(),
            out_refund_no: refund.out_refund_no.clone(),
            refund_id: None,
            amount: refund.amount,
            status: RefundStatus::Processing,
            reason: refund.reason.clone(),
            created_at,
        }
    }

    /// Whether this refund still counts against the order's refundable total.
    ///
    /// A failed refund releases the amount again; processing and succeeded
    /// refunds both hold it, so a double-submit cannot over-refund while the
    /// first attempt is still in flight.
    #[must_use]
    pub const fn holds_amount(&self) -> bool {
        !matches!(self.status, RefundStatus::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::RefundStatus::{Failed, Processing, Succeeded};
    use super::*;
    use crate::Currency;

    #[test]
    fn validates_refund_requests() {
        let order = RefundOrder::full("T-1", "R-1", Amount::cny(990));
        assert_eq!(order.validate(), Ok(()));

        let partial = RefundOrder::partial("T-1", "R-1", Amount::cny(1), Amount::cny(990));
        assert_eq!(partial.validate(), Ok(()));

        assert_eq!(
            RefundOrder::full("", "R-1", Amount::cny(1)).validate(),
            Err(PayError::InvalidRefund("out_trade_no must not be empty"))
        );
        assert_eq!(
            RefundOrder::full("T-1", " ", Amount::cny(1)).validate(),
            Err(PayError::InvalidRefund("out_refund_no must not be empty"))
        );
        assert_eq!(
            RefundOrder::full("T-1", "R-1", Amount::cny(0)).validate(),
            Err(PayError::InvalidRefund(
                "refund amount must be greater than zero"
            ))
        );
        assert_eq!(
            RefundOrder::partial("T-1", "R-1", Amount::cny(991), Amount::cny(990)).validate(),
            Err(PayError::InvalidRefund(
                "refund amount must not exceed the order total"
            ))
        );
        // The currency guard cannot fire while `Currency` has a single variant;
        // it exists so adding one cannot silently mix currencies in a refund.
        assert_eq!(
            RefundOrder::full("T-1", "R-1", Amount::from_minor(5, Currency::Cny)).validate(),
            Ok(())
        );
    }

    #[test]
    fn refund_status_machine_only_leaves_processing() {
        assert_eq!(Processing.transition(Succeeded), Ok(Succeeded));
        assert_eq!(Processing.transition(Failed), Ok(Failed));
        for (from, to) in [
            (Processing, Processing),
            (Succeeded, Failed),
            (Succeeded, Processing),
            (Failed, Succeeded),
            (Failed, Processing),
            (Failed, Failed),
        ] {
            assert_eq!(
                from.transition(to),
                Err(PayError::InvalidRefundTransition { from, to }),
                "{from} -> {to} must be rejected"
            );
        }
        assert!(Succeeded.is_terminal() && Failed.is_terminal() && !Processing.is_terminal());
    }

    #[test]
    fn refund_status_round_trips_through_str() {
        for status in [Processing, Succeeded, Failed] {
            assert_eq!(status.as_str().parse::<RefundStatus>(), Ok(status));
        }
        assert!("nope".parse::<RefundStatus>().is_err());
    }

    #[test]
    fn only_failed_refunds_release_the_amount() {
        let order = RefundOrder::full("T-1", "R-1", Amount::cny(990));
        let mut record = RefundRecord::new("mock", &order, SystemTime::UNIX_EPOCH);
        assert!(record.holds_amount());
        record.status = Succeeded;
        assert!(record.holds_amount());
        record.status = Failed;
        assert!(!record.holds_amount());
    }
}
