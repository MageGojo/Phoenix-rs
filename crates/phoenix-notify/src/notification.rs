use phoenix_mail::MessageBuilder;

use crate::ChannelKind;

/// A recipient of notifications (a user, an admin, a tenant, …).
///
/// Applications implement this on their own user type (for example the model
/// behind a `phoenix_auth::Principal` subject) — no framework type needs to
/// change. Identity is string-shaped on purpose: it fits `Principal::subject`,
/// integer primary keys via `to_string`, and UUIDs alike.
pub trait Notifiable: Send + Sync {
    /// Stable identity written to `notifications.notifiable_id` and used by
    /// [`Notifier::unread_for`](crate::Notifier::unread_for).
    fn notifiable_id(&self) -> String;

    /// Email address used by [`ChannelKind::Mail`].
    ///
    /// Returning `None` while a notification produces mail content makes the
    /// send fail closed with
    /// [`NotifyError::MissingMailAddress`](crate::NotifyError::MissingMailAddress).
    fn mail_address(&self) -> Option<String> {
        None
    }
}

/// One notification, deliverable over one or more [`ChannelKind`]s.
///
/// The converged Laravel `Notification` shape: `channels` declares the wanted
/// channels; `to_mail` / `to_database` build the per-channel representation.
/// A channel whose builder returns `None` is skipped for that send.
pub trait Notification: Send + Sync {
    /// Stable type written to the `notifications.type` column,
    /// e.g. `payment.succeeded` (static, like [`phoenix_plugin::Plugin::name`]).
    fn notification_type(&self) -> &'static str;

    /// Channels this notification wants to reach (duplicates are sent once).
    fn channels(&self) -> Vec<ChannelKind>;

    /// Mail representation, *without* recipient: the
    /// [`Notifier`](crate::Notifier) appends `to` from
    /// [`Notifiable::mail_address`] and builds the final
    /// [`phoenix_mail::Message`], so builder validation errors surface as
    /// [`NotifyError::Mail`](crate::NotifyError::Mail) instead of being
    /// swallowed. Return `None` to skip the mail channel.
    fn to_mail(&self, notifiable: &dyn Notifiable) -> Option<MessageBuilder> {
        let _ = notifiable;
        None
    }

    /// Database payload written to the `notifications.data` JSON column.
    /// Return `None` to skip the database channel.
    fn to_database(&self) -> Option<serde_json::Value> {
        None
    }
}
