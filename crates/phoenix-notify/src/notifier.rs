use std::sync::Arc;
use std::time::SystemTime;

use phoenix_mail::Mailer;

use crate::{
    ChannelKind, DatabaseNotification, Notifiable, Notification, NotificationStore, NotifyError,
};

/// Per-channel outcome of one [`Notifier::send`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SendSummary {
    sent: Vec<ChannelKind>,
    skipped: Vec<ChannelKind>,
    stored_id: Option<String>,
}

impl SendSummary {
    /// Channels that delivered, in processing order.
    #[must_use]
    pub fn sent(&self) -> &[ChannelKind] {
        &self.sent
    }

    /// Channels the notification requested but opted out of (`to_*` → `None`).
    #[must_use]
    pub fn skipped(&self) -> &[ChannelKind] {
        &self.skipped
    }

    /// Id of the stored database notification, when that channel delivered.
    #[must_use]
    pub fn stored_id(&self) -> Option<&str> {
        self.stored_id.as_deref()
    }

    /// Whether `channel` delivered.
    #[must_use]
    pub fn sent_via(&self, channel: ChannelKind) -> bool {
        self.sent.contains(&channel)
    }

    /// Whether `channel` was requested but skipped.
    #[must_use]
    pub fn skipped_via(&self, channel: ChannelKind) -> bool {
        self.skipped.contains(&channel)
    }
}

/// Assembled notification dispatcher for the mail and database channels.
///
/// Assemble once at startup and share (it is `Clone`):
///
/// ```
/// use std::sync::Arc;
/// use phoenix_mail::Mailer;
/// use phoenix_notify::{MemoryNotificationStore, Notifier};
///
/// let (mailer, _transport) = Mailer::memory();
/// let store = Arc::new(MemoryNotificationStore::new());
/// let notifier = Notifier::new().with_mailer(mailer).with_store(store);
/// # let _ = notifier;
/// ```
#[derive(Clone, Default)]
pub struct Notifier {
    mailer: Option<Mailer>,
    store: Option<Arc<dyn NotificationStore>>,
}

impl Notifier {
    /// Notifier with no channels configured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mail channel.
    #[must_use]
    pub fn with_mailer(mut self, mailer: Mailer) -> Self {
        self.mailer = Some(mailer);
        self
    }

    /// Configure the database channel.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn NotificationStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Send `notification` to `notifiable` over every requested channel.
    ///
    /// Duplicate channels are processed once (first-seen order). A channel
    /// whose `to_mail` / `to_database` returns `None` is skipped; a channel
    /// that produced content but has no configured backend fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`NotifyError::ChannelUnconfigured`] for a produced channel
    /// without a backend, [`NotifyError::MissingMailAddress`] when the mail
    /// channel finds no address, [`NotifyError::Mail`] for message build or
    /// transport failures, and store errors from the database channel.
    pub async fn send(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<SendSummary, NotifyError> {
        let mut summary = SendSummary::default();
        let mut seen: Vec<ChannelKind> = Vec::new();
        for channel in notification.channels() {
            if seen.contains(&channel) {
                continue;
            }
            seen.push(channel);
            match channel {
                ChannelKind::Mail => {
                    self.send_mail(notifiable, notification, &mut summary)
                        .await?;
                }
                ChannelKind::Database => {
                    self.store_database(notifiable, notification, &mut summary)
                        .await?;
                }
            }
        }
        Ok(summary)
    }

    /// Mark one stored notification as read (idempotent), returning the
    /// updated record.
    ///
    /// # Errors
    ///
    /// Returns [`NotifyError::ChannelUnconfigured`] without a store and
    /// [`NotifyError::NotificationNotFound`] for an unknown id.
    pub async fn mark_read(&self, id: &str) -> Result<DatabaseNotification, NotifyError> {
        self.store()?.mark_read(id, SystemTime::now()).await
    }

    /// Unread database notifications for one notifiable, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`NotifyError::ChannelUnconfigured`] without a store, plus any
    /// store failure.
    pub async fn unread_for(
        &self,
        notifiable_id: &str,
    ) -> Result<Vec<DatabaseNotification>, NotifyError> {
        self.store()?.unread_for(notifiable_id).await
    }

    fn store(&self) -> Result<&Arc<dyn NotificationStore>, NotifyError> {
        self.store.as_ref().ok_or(NotifyError::ChannelUnconfigured {
            channel: ChannelKind::Database,
        })
    }

    async fn send_mail(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        summary: &mut SendSummary,
    ) -> Result<(), NotifyError> {
        let Some(builder) = notification.to_mail(notifiable) else {
            summary.skipped.push(ChannelKind::Mail);
            return Ok(());
        };
        let mailer = self
            .mailer
            .as_ref()
            .ok_or(NotifyError::ChannelUnconfigured {
                channel: ChannelKind::Mail,
            })?;
        let address = notifiable
            .mail_address()
            .ok_or_else(|| NotifyError::MissingMailAddress {
                notifiable_id: notifiable.notifiable_id(),
            })?;
        let message = builder.to(address).build()?;
        mailer.send(message).await?;
        summary.sent.push(ChannelKind::Mail);
        Ok(())
    }

    async fn store_database(
        &self,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
        summary: &mut SendSummary,
    ) -> Result<(), NotifyError> {
        let Some(data) = notification.to_database() else {
            summary.skipped.push(ChannelKind::Database);
            return Ok(());
        };
        let store = self.store()?;
        let record = DatabaseNotification {
            id: uuid::Uuid::new_v4().to_string(),
            notifiable_id: notifiable.notifiable_id(),
            notification_type: notification.notification_type().to_owned(),
            data,
            read_at: None,
            created_at: SystemTime::now(),
        };
        let id = record.id.clone();
        store.insert(record).await?;
        summary.stored_id = Some(id);
        summary.sent.push(ChannelKind::Database);
        Ok(())
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Notifier")
            .field("mail", &self.mailer.is_some())
            .field("database", &self.store.is_some())
            .finish()
    }
}
