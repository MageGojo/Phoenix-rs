use std::time::SystemTime;

use phoenix_http::{Bytes, HeaderMap};
use serde::{Deserialize, Serialize};

use crate::{Amount, PayError, PaymentStatus};

/// A merchant-side payment order to hand to a provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateOrder {
    /// Merchant order number, unique per provider.
    pub out_trade_no: String,
    /// Amount in integer minor units.
    pub amount: Amount,
    /// Human-readable subject shown to the payer.
    pub subject: String,
}

impl CreateOrder {
    /// Convenience constructor.
    #[must_use]
    pub fn new(
        out_trade_no: impl Into<String>,
        amount: Amount,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            out_trade_no: out_trade_no.into(),
            amount,
            subject: subject.into(),
        }
    }

    /// Validate invariants shared by every provider.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidOrder`] for an empty `out_trade_no`, an empty
    /// `subject`, or a zero amount.
    pub fn validate(&self) -> Result<(), PayError> {
        if self.out_trade_no.trim().is_empty() {
            return Err(PayError::InvalidOrder("out_trade_no must not be empty"));
        }
        if self.subject.trim().is_empty() {
            return Err(PayError::InvalidOrder("subject must not be empty"));
        }
        if self.amount.is_zero() {
            return Err(PayError::InvalidOrder("amount must be greater than zero"));
        }
        Ok(())
    }
}

/// How the payer should complete the payment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PaymentAction {
    /// Render this text as a QR code (`WeChat` Native / Alipay F2F precreate).
    QrCode(String),
    /// Redirect the browser to this URL (H5 / page pay).
    Redirect(String),
    /// Opaque parameters for an app / mini-program SDK call.
    SdkParams(serde_json::Value),
}

/// Result of [`crate::PaymentProvider::create`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaymentIntent {
    /// Provider key that produced this intent.
    pub provider: String,
    /// Merchant order number.
    pub out_trade_no: String,
    /// Ordered amount.
    pub amount: Amount,
    /// What the caller should do next.
    pub action: PaymentAction,
}

/// Raw asynchronous notification as received on the webhook route.
#[derive(Clone, Debug)]
pub struct NotifyRequest {
    headers: HeaderMap,
    body: Bytes,
}

impl NotifyRequest {
    /// Wrap the webhook request pieces a provider needs for verification.
    #[must_use]
    pub fn new(headers: HeaderMap, body: Bytes) -> Self {
        Self { headers, body }
    }

    /// Build from a body only (tests / providers that ignore headers).
    #[must_use]
    pub fn from_body(body: impl Into<Bytes>) -> Self {
        Self::new(HeaderMap::new(), body.into())
    }

    /// All request headers (signature headers live here for `WeChat` v3).
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Raw request body.
    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Body as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidNotify`] when the body is not valid UTF-8.
    pub fn body_str(&self) -> Result<&str, PayError> {
        std::str::from_utf8(&self.body)
            .map_err(|_| PayError::InvalidNotify("notification body is not UTF-8".to_owned()))
    }
}

/// A verified, normalized asynchronous notification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotifyEvent {
    /// Merchant order number the event refers to.
    pub out_trade_no: String,
    /// Provider-side transaction id, when present.
    pub transaction_id: Option<String>,
    /// Status the provider reports; must be a legal transition target.
    pub status: PaymentStatus,
    /// Raw payload for auditing, persisted to `payments.notify_payload`.
    pub raw: String,
}

/// One persisted row of the `payments` table (or its in-memory stand-in).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaymentRecord {
    /// Provider key.
    pub provider: String,
    /// Merchant order number, unique per provider.
    pub out_trade_no: String,
    /// Ordered amount (integer minor units + currency).
    pub amount: Amount,
    /// Order subject.
    pub subject: String,
    /// Current state-machine status.
    pub status: PaymentStatus,
    /// Last verified notification payload, if any.
    pub notify_payload: Option<String>,
    /// When the order row was created.
    pub created_at: SystemTime,
    /// When the order first reached [`PaymentStatus::Paid`], if it ever did.
    ///
    /// This is what makes daily reconciliation possible: it is the timestamp a
    /// provider bill for a given day has to line up against.
    pub paid_at: Option<SystemTime>,
}

impl PaymentRecord {
    /// Fresh record in [`PaymentStatus::Created`] for a validated order.
    #[must_use]
    pub fn new(provider: impl Into<String>, order: &CreateOrder, created_at: SystemTime) -> Self {
        Self {
            provider: provider.into(),
            out_trade_no: order.out_trade_no.clone(),
            amount: order.amount,
            subject: order.subject.clone(),
            status: PaymentStatus::Created,
            notify_payload: None,
            created_at,
            paid_at: None,
        }
    }
}
