//! Laravel-style notifications for Phoenix (converged: mail + database).
//!
//! Applications implement [`Notification`] (what to say, on which channels)
//! and [`Notifiable`] (who receives it), assemble a [`Notifier`] from a
//! `phoenix_mail::Mailer` and a [`NotificationStore`], then `send`. The
//! [`NotifyFeature`] plugin ships the `notifications` table migration; list /
//! mark-read HTTP endpoints stay in the application.
//!
//! See `docs/NOTIFICATIONS.md`.

#![forbid(unsafe_code)]

mod channel;
mod db_store;
mod error;
mod feature;
mod notification;
mod notifier;
mod store;

pub use channel::ChannelKind;
pub use db_store::{DbNotificationStore, NotificationRow};
pub use error::NotifyError;
pub use feature::{NOTIFICATIONS_MIGRATION_ID, NotifyFeature, notifications_migration};
pub use notification::{Notifiable, Notification};
pub use notifier::{Notifier, SendSummary};
pub use store::{DatabaseNotification, MemoryNotificationStore, NotificationStore};

// Mail representation types come from `phoenix-mail`; aliased so notification
// code reads naturally and avoids clashing with other `Message` types.
pub use phoenix_mail::{Message as MailMessage, MessageBuilder as MailMessageBuilder};

/// Convenience re-exports for application code.
pub mod prelude {
    pub use crate::{
        ChannelKind, DatabaseNotification, DbNotificationStore, MailMessage, MailMessageBuilder,
        MemoryNotificationStore, Notifiable, Notification, NotificationRow, NotificationStore,
        Notifier, NotifyError, NotifyFeature, SendSummary,
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use phoenix_mail::{MailError, Mailer, MemoryTransport};
    use serde_json::json;

    use super::*;

    struct DemoUser {
        id: &'static str,
        email: Option<&'static str>,
    }

    impl Notifiable for DemoUser {
        fn notifiable_id(&self) -> String {
            self.id.to_owned()
        }

        fn mail_address(&self) -> Option<String> {
            self.email.map(str::to_owned)
        }
    }

    struct PaymentSucceeded {
        out_trade_no: &'static str,
        amount: i64,
        channels: Vec<ChannelKind>,
        mail: bool,
        database: bool,
    }

    impl PaymentSucceeded {
        fn both() -> Self {
            Self {
                out_trade_no: "PX-1001",
                amount: 990,
                channels: vec![ChannelKind::Mail, ChannelKind::Database],
                mail: true,
                database: true,
            }
        }
    }

    impl Notification for PaymentSucceeded {
        fn notification_type(&self) -> &'static str {
            "payment.succeeded"
        }

        fn channels(&self) -> Vec<ChannelKind> {
            self.channels.clone()
        }

        fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessageBuilder> {
            self.mail.then(|| {
                MailMessage::builder()
                    .from("noreply@example.com")
                    .subject("Payment received")
                    .text_body(format!("Order {} paid", self.out_trade_no))
            })
        }

        fn to_database(&self) -> Option<serde_json::Value> {
            self.database.then(|| {
                json!({
                    "out_trade_no": self.out_trade_no,
                    "amount": self.amount,
                })
            })
        }
    }

    fn assembled() -> (Notifier, MemoryTransport, Arc<MemoryNotificationStore>) {
        let (mailer, transport) = Mailer::memory();
        let store = Arc::new(MemoryNotificationStore::new());
        let notifier = Notifier::new()
            .with_mailer(mailer)
            .with_store(Arc::clone(&store) as Arc<dyn NotificationStore>);
        (notifier, transport, store)
    }

    #[tokio::test]
    async fn dual_channel_send_reaches_mail_and_database() {
        let (notifier, transport, store) = assembled();
        let user = DemoUser {
            id: "user-1",
            email: Some("user@example.com"),
        };

        let summary = notifier
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect("send");

        assert!(summary.sent_via(ChannelKind::Mail));
        assert!(summary.sent_via(ChannelKind::Database));
        assert!(summary.skipped().is_empty());

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].from().as_str(), "noreply@example.com");
        assert_eq!(sent[0].to()[0].as_str(), "user@example.com");
        assert_eq!(sent[0].subject(), "Payment received");
        assert_eq!(sent[0].text_body(), Some("Order PX-1001 paid"));

        let records = store.all();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(summary.stored_id(), Some(record.id.as_str()));
        assert_eq!(record.notifiable_id, "user-1");
        assert_eq!(record.notification_type, "payment.succeeded");
        assert_eq!(record.data["out_trade_no"], "PX-1001");
        assert_eq!(record.data["amount"], 990);
        assert!(record.read_at.is_none());
    }

    #[tokio::test]
    async fn none_representation_skips_that_channel_only() {
        let (notifier, transport, store) = assembled();
        let user = DemoUser {
            id: "user-2",
            email: Some("user2@example.com"),
        };

        let no_mail = PaymentSucceeded {
            mail: false,
            ..PaymentSucceeded::both()
        };
        let summary = notifier.send(&user, &no_mail).await.expect("send");
        assert!(summary.skipped_via(ChannelKind::Mail));
        assert!(summary.sent_via(ChannelKind::Database));
        assert!(transport.is_empty());
        assert_eq!(store.len(), 1);

        let no_database = PaymentSucceeded {
            database: false,
            ..PaymentSucceeded::both()
        };
        let summary = notifier.send(&user, &no_database).await.expect("send");
        assert!(summary.sent_via(ChannelKind::Mail));
        assert!(summary.skipped_via(ChannelKind::Database));
        assert!(summary.stored_id().is_none());
        assert_eq!(transport.len(), 1);
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn duplicate_channels_deliver_once() {
        let (notifier, transport, store) = assembled();
        let user = DemoUser {
            id: "user-3",
            email: Some("user3@example.com"),
        };
        let notification = PaymentSucceeded {
            channels: vec![
                ChannelKind::Mail,
                ChannelKind::Mail,
                ChannelKind::Database,
                ChannelKind::Database,
            ],
            ..PaymentSucceeded::both()
        };

        let summary = notifier.send(&user, &notification).await.expect("send");
        assert_eq!(summary.sent().len(), 2);
        assert_eq!(transport.len(), 1);
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn mark_read_and_unread_for_roundtrip() {
        let (notifier, _transport, _store) = assembled();
        let user = DemoUser {
            id: "user-4",
            email: Some("user4@example.com"),
        };

        let first = notifier
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect("first");
        let second = notifier
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect("second");

        let unread = notifier.unread_for("user-4").await.expect("unread");
        assert_eq!(unread.len(), 2);
        assert_eq!(unread[0].id, first.stored_id().expect("first id"));
        assert!(
            notifier
                .unread_for("nobody")
                .await
                .expect("empty")
                .is_empty()
        );

        let marked = notifier
            .mark_read(first.stored_id().expect("id"))
            .await
            .expect("mark read");
        assert!(marked.is_read());

        // Idempotent: the original read_at is kept.
        let again = notifier
            .mark_read(first.stored_id().expect("id"))
            .await
            .expect("mark read again");
        assert_eq!(again.read_at, marked.read_at);

        let unread = notifier.unread_for("user-4").await.expect("unread");
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, second.stored_id().expect("second id"));

        let missing = notifier.mark_read("does-not-exist").await;
        assert_eq!(
            missing,
            Err(NotifyError::NotificationNotFound {
                id: "does-not-exist".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn missing_mail_address_fails_closed() {
        let (notifier, transport, store) = assembled();
        let user = DemoUser {
            id: "user-5",
            email: None,
        };

        let error = notifier
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect_err("no address");
        assert_eq!(
            error,
            NotifyError::MissingMailAddress {
                notifiable_id: "user-5".to_owned(),
            }
        );
        assert!(transport.is_empty());
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn unconfigured_channels_fail_closed() {
        let user = DemoUser {
            id: "user-6",
            email: Some("user6@example.com"),
        };

        let mail_only = Notifier::new().with_mailer(Mailer::memory().0);
        let error = mail_only
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect_err("no store");
        assert_eq!(
            error,
            NotifyError::ChannelUnconfigured {
                channel: ChannelKind::Database,
            }
        );

        let store_only =
            Notifier::new().with_store(Arc::new(MemoryNotificationStore::new()) as Arc<_>);
        let error = store_only
            .send(&user, &PaymentSucceeded::both())
            .await
            .expect_err("no mailer");
        assert_eq!(
            error,
            NotifyError::ChannelUnconfigured {
                channel: ChannelKind::Mail,
            }
        );

        let bare = Notifier::new();
        assert!(bare.unread_for("user-6").await.is_err());
        assert!(bare.mark_read("any").await.is_err());
    }

    #[tokio::test]
    async fn mail_build_errors_surface() {
        // The notification "forgets" the from address; the builder error must
        // surface as NotifyError::Mail instead of being swallowed.
        struct BrokenMail;

        impl Notification for BrokenMail {
            fn notification_type(&self) -> &'static str {
                "broken.mail"
            }

            fn channels(&self) -> Vec<ChannelKind> {
                vec![ChannelKind::Mail]
            }

            fn to_mail(&self, _notifiable: &dyn Notifiable) -> Option<MailMessageBuilder> {
                Some(MailMessage::builder().subject("no from"))
            }
        }

        let (notifier, transport, _store) = assembled();
        let user = DemoUser {
            id: "user-7",
            email: Some("user7@example.com"),
        };
        let error = notifier
            .send(&user, &BrokenMail)
            .await
            .expect_err("missing from");
        assert_eq!(error, NotifyError::Mail(MailError::MissingFrom));
        assert!(transport.is_empty());
    }

    #[test]
    fn feature_registers_only_the_notifications_migration() {
        use phoenix_plugin::FeatureSet;

        let parts = FeatureSet::new()
            .plugin(NotifyFeature::new())
            .expect("install")
            .into_parts();
        assert!(parts.commands.is_empty());
        assert!(parts.routes.is_empty());
        assert_eq!(parts.migrations.len(), 1);

        let migration = &parts.migrations[0];
        assert_eq!(migration.id(), NOTIFICATIONS_MIGRATION_ID);
        assert_eq!(migration.name(), "create notifications table");
        // Deterministic checksum guards accidental SQL drift.
        assert_eq!(notifications_migration().checksum(), migration.checksum());
    }
}
