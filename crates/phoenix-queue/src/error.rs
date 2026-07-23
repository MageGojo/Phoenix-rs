//! Queue and job error types.

use thiserror::Error;

use crate::JobId;

/// Errors produced by queue backends and the worker loop.
#[derive(Debug, Error)]
pub enum QueueError {
    /// The referenced job is not present in the backend.
    #[error("job not found: {0}")]
    NotFound(JobId),
    /// The job exists but is not in a state that allows the requested operation.
    #[error("job {id} is in an invalid state for this operation")]
    InvalidState {
        /// Job identifier.
        id: JobId,
    },
    /// Payload serialization failed.
    #[error("failed to serialize job payload: {0}")]
    Serialize(#[from] serde_json::Error),
    /// Backend-specific failure (reserved for future Redis / durable adapters).
    #[error("queue backend error: {0}")]
    Backend(String),
}

/// Errors returned by job handlers.
#[derive(Debug, Error)]
pub enum JobError {
    /// Transient failure; the worker should retry when attempts remain.
    #[error("job failed (retryable): {0}")]
    Retryable(String),
    /// Permanent failure; the worker should dead-letter immediately.
    #[error("job failed (permanent): {0}")]
    Permanent(String),
}

impl JobError {
    /// Construct a retryable failure.
    #[must_use]
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable(message.into())
    }

    /// Construct a permanent failure.
    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self::Permanent(message.into())
    }

    /// Whether the worker should schedule a retry rather than dead-letter.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}
