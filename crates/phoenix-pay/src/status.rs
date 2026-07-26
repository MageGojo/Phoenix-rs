use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::PayError;

/// Payment order lifecycle.
///
/// ```text
/// Created ──> Pending ──> Paid ⇄ Refunding ──> Refunded
///    │           │  └───> Failed
///    └───────────┴──────> Closed
/// ```
///
/// The refund arm is bidirectional on purpose: an order enters `Refunding` when
/// a refund is accepted, reaches `Refunded` once the succeeded refunds cover the
/// order total, and falls back to `Paid` when every refund failed — the money
/// never left, so the order really is paid again. Every transition not drawn
/// above is rejected with [`PayError::InvalidTransition`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    /// Order row persisted; provider not called yet.
    #[default]
    Created,
    /// Provider accepted the order; waiting for the payer.
    Pending,
    /// Asynchronous notification confirmed payment.
    Paid,
    /// Provider reported a definitive failure.
    Failed,
    /// Closed before payment (timeout / manual close).
    Closed,
    /// Refund requested (reserved, no gateway yet).
    Refunding,
    /// Refund settled (reserved, no gateway yet).
    Refunded,
}

impl PaymentStatus {
    /// Stable lowercase name, used in the `payments.status` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Failed => "failed",
            Self::Closed => "closed",
            Self::Refunding => "refunding",
            Self::Refunded => "refunded",
        }
    }

    /// Whether no further transition can leave this status.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Closed | Self::Refunded)
    }

    /// Whether the state machine allows `self -> next`.
    #[must_use]
    pub const fn can_transition(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Created, Self::Pending | Self::Closed)
                | (Self::Pending, Self::Paid | Self::Failed | Self::Closed)
                | (Self::Paid, Self::Refunding)
                | (Self::Refunding, Self::Refunded | Self::Paid)
        )
    }

    /// Validate and perform a transition.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::InvalidTransition`] when the move is not allowed
    /// (including no-op transitions to the same status).
    pub fn transition(self, next: Self) -> Result<Self, PayError> {
        if self.can_transition(next) {
            Ok(next)
        } else {
            Err(PayError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for PaymentStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for PaymentStatus {
    type Err = PayError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "created" => Ok(Self::Created),
            "pending" => Ok(Self::Pending),
            "paid" => Ok(Self::Paid),
            "failed" => Ok(Self::Failed),
            "closed" => Ok(Self::Closed),
            "refunding" => Ok(Self::Refunding),
            "refunded" => Ok(Self::Refunded),
            _ => Err(PayError::InvalidNotify(format!(
                "unknown payment status `{value}`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PaymentStatus::{Closed, Created, Failed, Paid, Pending, Refunded, Refunding};
    use super::*;

    #[test]
    fn allows_documented_transitions() {
        assert_eq!(Created.transition(Pending), Ok(Pending));
        assert_eq!(Created.transition(Closed), Ok(Closed));
        assert_eq!(Pending.transition(Paid), Ok(Paid));
        assert_eq!(Pending.transition(Failed), Ok(Failed));
        assert_eq!(Pending.transition(Closed), Ok(Closed));
        assert_eq!(Paid.transition(Refunding), Ok(Refunding));
        assert_eq!(Refunding.transition(Refunded), Ok(Refunded));
        // Every refund failed: the money never left, so the order is paid again.
        assert_eq!(Refunding.transition(Paid), Ok(Paid));
    }

    #[test]
    fn rejects_illegal_transitions() {
        for (from, to) in [
            (Created, Paid),
            (Created, Created),
            (Paid, Paid),
            (Paid, Failed),
            (Paid, Created),
            (Failed, Paid),
            (Closed, Pending),
            (Refunded, Refunding),
            (Refunding, Refunding),
            (Refunding, Pending),
            (Refunding, Closed),
            (Pending, Created),
        ] {
            assert_eq!(
                from.transition(to),
                Err(PayError::InvalidTransition { from, to }),
                "{from} -> {to} must be rejected"
            );
        }
    }

    #[test]
    fn terminal_states_have_no_exits() {
        for terminal in [Failed, Closed, Refunded] {
            assert!(terminal.is_terminal());
            for to in [Created, Pending, Paid, Failed, Closed, Refunding, Refunded] {
                assert!(!terminal.can_transition(to));
            }
        }
    }

    #[test]
    fn round_trips_through_str() {
        for status in [Created, Pending, Paid, Failed, Closed, Refunding, Refunded] {
            assert_eq!(status.as_str().parse::<PaymentStatus>(), Ok(status));
        }
        assert!("nope".parse::<PaymentStatus>().is_err());
    }
}
