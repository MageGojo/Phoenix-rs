//! High-level queue facade over a [`QueueBackend`].

use std::sync::Arc;

use serde::Serialize;

use crate::{JobEnvelope, PushResult, QueueBackend, QueueError};

/// Default maximum attempts when callers omit an explicit value.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Options for [`Queue::push_json`] / [`Queue::dispatch`].
#[derive(Clone, Debug, Default)]
pub struct PushOptions {
    /// Maximum reserves before dead-lettering. Defaults to [`DEFAULT_MAX_ATTEMPTS`].
    pub max_attempts: Option<u32>,
    /// Optional dedupe key while the job remains in-flight.
    pub idempotency_key: Option<String>,
}

impl PushOptions {
    /// Start with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max attempts (clamped to at least 1 by the envelope).
    #[must_use]
    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = Some(max_attempts);
        self
    }

    /// Set the idempotency key.
    #[must_use]
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }
}

/// Thin facade that builds envelopes and delegates to a backend.
#[derive(Clone, Debug)]
pub struct Queue<B> {
    backend: Arc<B>,
    default_max_attempts: u32,
}

impl<B> Queue<B>
where
    B: QueueBackend,
{
    /// Wrap an existing backend.
    #[must_use]
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            default_max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    /// Override the default max attempts used when options omit it.
    #[must_use]
    pub fn with_default_max_attempts(mut self, max_attempts: u32) -> Self {
        self.default_max_attempts = max_attempts.max(1);
        self
    }

    /// Borrow the shared backend.
    #[must_use]
    pub fn backend(&self) -> &Arc<B> {
        &self.backend
    }

    /// Serialize `payload` to JSON and enqueue under `name`.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Serialize`] when the payload cannot be encoded, or a
    /// backend error from [`QueueBackend::push`].
    pub async fn push_json(
        &self,
        name: impl Into<String>,
        payload: impl Serialize,
        options: PushOptions,
    ) -> Result<PushResult, QueueError> {
        let value = serde_json::to_value(payload)?;
        let max_attempts = options.max_attempts.unwrap_or(self.default_max_attempts);
        let job = JobEnvelope::new(name, value, max_attempts, options.idempotency_key);
        self.backend.push(job).await
    }

    /// Convenience alias for [`Self::push_json`] with default options.
    ///
    /// # Errors
    ///
    /// Same as [`Self::push_json`].
    pub async fn dispatch(
        &self,
        name: impl Into<String>,
        payload: impl Serialize,
    ) -> Result<PushResult, QueueError> {
        self.push_json(name, payload, PushOptions::default()).await
    }

    /// Push with an explicit idempotency key.
    ///
    /// # Errors
    ///
    /// Same as [`Self::push_json`].
    pub async fn dispatch_once(
        &self,
        name: impl Into<String>,
        payload: impl Serialize,
        idempotency_key: impl Into<String>,
    ) -> Result<PushResult, QueueError> {
        self.push_json(
            name,
            payload,
            PushOptions::new().idempotency_key(idempotency_key),
        )
        .await
    }
}
