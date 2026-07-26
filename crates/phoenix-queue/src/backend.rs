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
///
/// # Delivery guarantee
///
/// Backends provide **at-least-once** delivery. A reserved job that is never
/// acked/failed/dead-lettered (e.g. its worker crashed) becomes reservable
/// again once its *visibility timeout* elapses — see [`reclaim_expired`]. A
/// job may therefore be handed to a handler more than once; keep handlers
/// idempotent (the `idempotency_key` on [`JobEnvelope`] is the dedupe hint for
/// enqueue, not for execution).
///
/// [`reclaim_expired`]: QueueBackend::reclaim_expired
pub trait QueueBackend: Send + Sync {
    /// Enqueue `job`, honouring its idempotency key when present.
    fn push(&self, job: JobEnvelope)
    -> impl Future<Output = Result<PushResult, QueueError>> + Send;

    /// Claim the next runnable job (`available_at <= now`), incrementing `attempts`.
    ///
    /// A backend configured with a visibility timeout also makes the claimed
    /// job invisible for that window; if it is not acked/failed/dead-lettered
    /// before the window closes it becomes reservable again.
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

    /// Return reserved jobs whose visibility timeout has elapsed to the ready
    /// set so another worker can pick them up, yielding the number reclaimed.
    ///
    /// This is what turns a crashed / stalled worker's in-flight job back into
    /// a runnable one (at-least-once delivery). Backends without a visibility
    /// timeout leave the default no-op; durable backends should reclaim lazily
    /// on [`reserve`](QueueBackend::reserve) as well so any active worker
    /// recovers stuck jobs without a dedicated sweeper.
    fn reclaim_expired(&self) -> impl Future<Output = Result<usize, QueueError>> + Send {
        async { Ok(0) }
    }

    /// Remove expired idempotency reservations (no-op for backends that free keys on terminal states).
    fn purge_expired_idempotency(&self) -> impl Future<Output = Result<usize, QueueError>> + Send {
        async { Ok(0) }
    }
}
