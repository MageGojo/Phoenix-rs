//! Outbound email for Phoenix.
//!
//! Provides a validated [`Message`] builder, pluggable [`MailTransport`],
//! and a [`Mailer`] facade. The first release ships [`MemoryTransport`] for
//! tests; real SMTP is intentionally deferred (`// future SmtpTransport`).
//!
//! See `docs/MAIL.md` and `docs/QUEUE_MAIL_CONSOLE.md`.

#![forbid(unsafe_code)]

mod address;
mod error;
mod message;
mod transport;

pub use address::Address;
pub use error::MailError;
pub use message::{Message, MessageBuilder};
pub use transport::{MailTransport, Mailer, MemoryTransport};

/// Convenience re-exports for application code.
pub mod prelude {
    pub use crate::{
        Address, MailError, MailTransport, Mailer, MemoryTransport, Message, MessageBuilder,
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn sample_message() -> Message {
        Message::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .cc("copy@example.com")
            .subject("Welcome")
            .text_body("Hello in text")
            .html_body("<p>Hello in <strong>HTML</strong></p>")
            .build()
            .expect("sample message")
    }

    #[tokio::test]
    async fn memory_transport_records_successful_send() {
        let transport = MemoryTransport::new();
        let mailer = Mailer::new(Arc::new(transport.clone()));
        let message = sample_message();

        mailer.send(message.clone()).await.expect("send");

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], message);
        assert_eq!(sent[0].from().as_str(), "noreply@example.com");
        assert_eq!(sent[0].to()[0].as_str(), "user@example.com");
        assert_eq!(sent[0].cc()[0].as_str(), "copy@example.com");
    }

    #[tokio::test]
    async fn mailer_memory_helper_shares_recorder() {
        let (mailer, transport) = Mailer::memory();
        mailer
            .send(sample_message())
            .await
            .expect("send via helper");
        assert_eq!(transport.len(), 1);
        transport.clear();
        assert!(transport.is_empty());
    }

    #[test]
    fn rejects_crlf_subject_header_injection() {
        let error = Message::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .subject("Hi\r\nBcc: attacker@evil.test")
            .build()
            .expect_err("CRLF subject");
        assert_eq!(error, MailError::HeaderInjection { field: "subject" });
    }

    #[test]
    fn rejects_crlf_in_address_fields() {
        let error = Message::builder()
            .from("noreply@example.com")
            .to("user@example.com\nbcc:attacker@evil.test")
            .subject("Hi")
            .build()
            .expect_err("CRLF address");
        assert_eq!(error, MailError::HeaderInjection { field: "to" });
    }

    #[test]
    fn rejects_empty_recipients() {
        let error = Message::builder()
            .from("noreply@example.com")
            .subject("Hi")
            .text_body("body")
            .build()
            .expect_err("no recipients");
        assert_eq!(error, MailError::NoRecipients);
    }

    #[test]
    fn allows_html_and_text_together() {
        let message = sample_message();
        assert_eq!(message.text_body(), Some("Hello in text"));
        assert_eq!(
            message.html_body(),
            Some("<p>Hello in <strong>HTML</strong></p>")
        );
        // Body newlines are fine; only headers are guarded.
        let with_newlines = Message::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .subject("Hi")
            .text_body("line1\nline2")
            .html_body("<p>line1<br>\nline2</p>")
            .build()
            .expect("bodies may contain newlines");
        assert!(with_newlines.text_body().unwrap().contains('\n'));
    }

    #[test]
    fn bcc_alone_counts_as_recipient() {
        let message = Message::builder()
            .from("noreply@example.com")
            .bcc("hidden@example.com")
            .subject("Secret")
            .build()
            .expect("bcc-only ok");
        assert!(message.to().is_empty());
        assert_eq!(message.bcc().len(), 1);
    }
}
