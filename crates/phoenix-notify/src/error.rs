use phoenix_mail::MailError;
use thiserror::Error;

use crate::ChannelKind;

/// Stable errors for notification dispatch and the notification store.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum NotifyError {
    /// A notification produced content for a channel the
    /// [`Notifier`](crate::Notifier) was not assembled with (fail closed).
    #[error("notification channel `{channel}` is not configured on this notifier")]
    ChannelUnconfigured {
        /// The requested but unconfigured channel.
        channel: ChannelKind,
    },
    /// The mail channel was requested but
    /// [`Notifiable::mail_address`](crate::Notifiable::mail_address) returned `None`.
    #[error("notifiable `{notifiable_id}` has no mail address")]
    MissingMailAddress {
        /// Identity of the notifiable that lacks an address.
        notifiable_id: String,
    },
    /// Building or delivering the mail message failed.
    #[error(transparent)]
    Mail(#[from] MailError),
    /// The store already holds a notification with this id.
    #[error("duplicate notification id `{id}`")]
    DuplicateNotification {
        /// The conflicting notification id.
        id: String,
    },
    /// The store holds no notification with this id.
    #[error("notification `{id}` not found")]
    NotificationNotFound {
        /// The unknown notification id.
        id: String,
    },
    /// The configured [`NotificationStore`](crate::NotificationStore) failed.
    #[error("notification store error: {0}")]
    Store(String),
}
