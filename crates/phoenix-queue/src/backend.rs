//! Backend trait and push result types.

use std::{future::Future, time::SystemTime};

use crate::{JobEnvelope, JobId, QueueError};

/// Outcome of pushing a job into a backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushResult {
    /// A new job was enqueued.
    Created(JobId),
    /// An in-flight job with the same idempotency key already exists.
    Existing(JobId),
}

impl PushResult {
    /// Return the job id regardless of created vs existing.
    #[must_use]
    pub const fn job_id(self) -> JobId {
        match self {
            Self::Created(id) | Self::Existing(id) => id,
        }
    }

    /// Whether this push created a brand-new job.
    #[must_use]
    pub const fn is_created(self) -> bool {
        matches!(self, Self::Created(_))
    }
}

/// Durable or in-memory store for job envelopes.
///
/// Implementations must treat jobs with the same non-empty `idempotency_key` as
/// duplicates **while that job is still queued or reserved**. After [`ack`] or
/// [`dead_letter`], the key may be reused. See [`crate::MemoryQueue`].
pub trait QueueBackend: Send + Sync {
    /// Enqueue `job`, honouring its idempotency key when present.
    fn push(&self, job: JobEnvelope)
    -> impl Future<Output = Result<PushResult, QueueError>> + Send;

    /// Claim the next runnable job (`available_at <= now`), incrementing `attempts`.
    fn reserve(&self) -> impl Future<Output = Result<Option<JobEnvelope>, QueueError>> + Send;

    /// Mark a reserved job as successfully completed and free its idempotency key.
    fn ack(&self, id: &JobId) -> impl Future<Output = Result<(), QueueError>> + Send;

    /// Return a reserved job to the queue with a new visibility time.
    fn fail(
        &self,
        id: &JobId,
        available_at: SystemTime,
    ) -> impl Future<Output = Result<(), QueueError>> + Send;

    /// Move a reserved job to the dead-letter set and free its idempotency key.
    fn dead_letter(&self, id: &JobId) -> impl Future<Output = Result<(), QueueError>> + Send;

    /// Remove expired idempotency reservations (no-op for backends that free keys on terminal states).
    fn purge_expired_idempotency(&self) -> impl Future<Output = Result<usize, QueueError>> + Send {
        async { Ok(0) }
    }
}
