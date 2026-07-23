use std::sync::{Arc, Mutex};

use phoenix_http::BoxFuture;

use crate::{MailError, Message};

/// Pluggable delivery backend for outbound mail.
///
/// Applications may implement this trait for SMTP, API providers, or test doubles.
/// Real SMTP is intentionally out of scope for the first release
/// (`// future SmtpTransport`).
pub trait MailTransport: Send + Sync {
    /// Deliver `message` asynchronously.
    ///
    /// Implementors that need to retain the message across an await point should
    /// clone it; [`phoenix_http::BoxFuture`] is `'static`.
    fn send(&self, message: &Message) -> BoxFuture<Result<(), MailError>>;
}

/// In-memory transport that records every accepted message for assertions.
#[derive(Clone, Default)]
pub struct MemoryTransport {
    sent: Arc<Mutex<Vec<Message>>>,
}

impl MemoryTransport {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of messages accepted so far (oldest first).
    #[must_use]
    pub fn sent(&self) -> Vec<Message> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Drop all recorded messages.
    pub fn clear(&self) {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Number of recorded messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether no messages have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl MailTransport for MemoryTransport {
    fn send(&self, message: &Message) -> BoxFuture<Result<(), MailError>> {
        let message = message.clone();
        let sent = Arc::clone(&self.sent);
        Box::pin(async move {
            sent.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message);
            Ok(())
        })
    }
}

/// Application-facing mail sender wrapping a shared [`MailTransport`].
#[derive(Clone)]
pub struct Mailer {
    transport: Arc<dyn MailTransport>,
}

impl Mailer {
    /// Bind a transport implementation.
    #[must_use]
    pub fn new(transport: Arc<dyn MailTransport>) -> Self {
        Self { transport }
    }

    /// Convenience constructor for tests and local development.
    #[must_use]
    pub fn memory() -> (Self, MemoryTransport) {
        let transport = MemoryTransport::new();
        let mailer = Self::new(Arc::new(transport.clone()));
        (mailer, transport)
    }

    /// Send a fully built [`Message`].
    ///
    /// # Errors
    ///
    /// Propagates [`MailError`] from the underlying transport.
    pub async fn send(&self, message: Message) -> Result<(), MailError> {
        self.transport.send(&message).await
    }
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Mailer").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for MemoryTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryTransport")
            .field("sent", &self.len())
            .finish()
    }
}
