use thiserror::Error;

/// Stable errors for address validation, message construction, and transport.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MailError {
    /// The address failed basic mailbox validation.
    #[error("invalid email address: {reason}")]
    InvalidAddress {
        /// Human-readable validation failure.
        reason: &'static str,
    },
    /// A header field contained CR, LF, or other forbidden control characters.
    #[error("header injection detected in `{field}`")]
    HeaderInjection {
        /// Field name that contained the injection (`subject`, `from`, …).
        field: &'static str,
    },
    /// [`Message`](crate::Message) was built without a `From` address.
    #[error("message is missing a from address")]
    MissingFrom,
    /// [`Message`](crate::Message) had no `To`, `Cc`, or `Bcc` recipients.
    #[error("message has no recipients")]
    NoRecipients,
    /// The configured [`MailTransport`](crate::MailTransport) failed to deliver.
    #[error("mail transport error: {0}")]
    Transport(String),
}
