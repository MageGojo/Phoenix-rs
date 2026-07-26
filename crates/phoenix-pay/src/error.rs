use thiserror::Error;

use crate::refund::RefundStatus;
use crate::status::PaymentStatus;

/// Stable payment failure categories.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PayError {
    /// The requested provider key is not registered on the [`crate::PayManager`].
    #[error("unknown payment provider `{0}`")]
    UnknownProvider(String),
    /// Order validation failed before reaching any provider.
    #[error("invalid payment order: {0}")]
    InvalidOrder(&'static str),
    /// An order with the same `provider + out_trade_no` already exists.
    #[error("duplicate payment order `{out_trade_no}` for provider `{provider}`")]
    DuplicateOrder {
        provider: String,
        out_trade_no: String,
    },
    /// No stored order matches `provider + out_trade_no`.
    #[error("payment order `{out_trade_no}` not found for provider `{provider}`")]
    OrderNotFound {
        provider: String,
        out_trade_no: String,
    },
    /// The requested status change violates the payment state machine.
    #[error("invalid payment status transition {from} -> {to}")]
    InvalidTransition {
        from: PaymentStatus,
        to: PaymentStatus,
    },
    /// Refund validation failed before reaching any provider.
    #[error("invalid refund: {0}")]
    InvalidRefund(&'static str),
    /// A refund with the same `provider + out_refund_no` already exists.
    #[error("duplicate refund `{out_refund_no}` for provider `{provider}`")]
    DuplicateRefund {
        provider: String,
        out_refund_no: String,
    },
    /// No stored refund matches `provider + out_refund_no`.
    #[error("refund `{out_refund_no}` not found for provider `{provider}`")]
    RefundNotFound {
        provider: String,
        out_refund_no: String,
    },
    /// The requested change violates the refund state machine.
    #[error("invalid refund status transition {from} -> {to}")]
    InvalidRefundTransition {
        from: RefundStatus,
        to: RefundStatus,
    },
    /// The order cannot be refunded for more than it was paid.
    #[error(
        "refund of {requested} exceeds the refundable {refundable} left on order `{out_trade_no}`"
    )]
    RefundExceedsOrder {
        out_trade_no: String,
        requested: crate::Amount,
        refundable: crate::Amount,
    },
    /// A provider bill could not be fetched or parsed.
    #[error("reconciliation failed: {0}")]
    Reconcile(String),
    /// Arithmetic across two different currencies.
    #[error("currency mismatch: {left} vs {right}")]
    CurrencyMismatch {
        left: &'static str,
        right: &'static str,
    },
    /// Integer overflow while combining amounts.
    #[error("payment amount overflow")]
    AmountOverflow,
    /// The asynchronous notification could not be parsed or verified.
    #[error("invalid payment notification: {0}")]
    InvalidNotify(String),
    /// Channel configuration or key material problem (bad PEM, missing file,
    /// wrong `APIv3` key length, ...). Fix the deployment, not the request.
    #[error("payment gateway config error: {0}")]
    Config(String),
    /// Transport or protocol failure while talking to the real gateway,
    /// including gateway responses whose signature does not verify.
    #[error("payment gateway error: {0}")]
    Gateway(String),
    /// Storage backend failure.
    #[error("payment store error: {0}")]
    Store(String),
    /// Feature seam that is intentionally not implemented yet (e.g. refunds,
    /// or `close` on providers without a close API); see `docs/PAYMENTS.md`
    /// for the follow-up list.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
}
