//! In-process queue backend.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, SystemTime},
};

use crate::{JobEnvelope, JobId, PushResult, QueueBackend, QueueError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JobState {
    Queued,
    Reserved,
    Dead,
}

struct StoredJob {
    envelope: JobEnvelope,
    state: JobState,
    /// Instant after which a reserved job may be reclaimed (visibility
    /// timeout). `None` means "reserved indefinitely" — the default when no
    /// visibility timeout is configured.
    reserved_until: Option<SystemTime>,
}

/// Process-local FIFO queue with idempotency and dead-letter support.
///
/// # Idempotency
///
/// When `idempotency_key` is set and a job with that key is still **queued or
/// reserved**, [`push`](QueueBackend::push) returns [`PushResult::Existing`]
/// with the original id (payload is not replaced). After [`ack`](QueueBackend::ack)
/// or [`dead_letter`](QueueBackend::dead_letter), the key is released and may be
/// reused.
///
/// # Visibility timeout
///
/// By default a reserved job stays reserved until it is acked, failed, or
/// dead-lettered. Configure a visibility timeout with
/// [`with_visibility_timeout`](Self::with_visibility_timeout) to mirror the
/// durable backends: a job that is not resolved within the window is returned
/// to the ready set (lazily on the next [`reserve`](QueueBackend::reserve), or
/// eagerly via [`reclaim_expired`](QueueBackend::reclaim_expired)).
#[derive(Default)]
pub struct MemoryQueue {
    inner: Mutex<Inner>,
    visibility_timeout: Option<Duration>,
}

#[derive(Default)]
struct Inner {
    jobs: HashMap<JobId, StoredJob>,
    /// Ready queue ordered by push time; visibility filtered on reserve.
    ready: VecDeque<JobId>,
    idempotency: HashMap<String, JobId>,
    dead_letters: Vec<JobEnvelope>,
}

impl MemoryQueue {
    /// Create an empty in-memory queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve jobs invisibly for `timeout`; unresolved reservations become
    /// reservable again after it elapses (at-least-once delivery).
    #[must_use]
    pub const fn with_visibility_timeout(mut self, timeout: Duration) -> Self {
        self.visibility_timeout = Some(timeout);
        self
    }

    /// Snapshot of dead-lettered envelopes (oldest first).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn dead_letters(&self) -> Vec<JobEnvelope> {
        self.inner
            .lock()
            .expect("memory queue poisoned")
            .dead_letters
            .clone()
    }

    /// Number of jobs currently queued or reserved.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("memory queue poisoned")
            .jobs
            .values()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Reserved))
            .count()
    }

    /// Whether there are no queued or reserved jobs.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl QueueBackend for MemoryQueue {
    async fn push(&self, job: JobEnvelope) -> Result<PushResult, QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");

        if let Some(key) = job.idempotency_key.as_deref() {
            if let Some(existing_id) = inner.idempotency.get(key).copied() {
                if let Some(stored) = inner.jobs.get(&existing_id)
                    && matches!(stored.state, JobState::Queued | JobState::Reserved)
                {
                    return Ok(PushResult::Existing(existing_id));
                }
                inner.idempotency.remove(key);
            }
            inner.idempotency.insert(key.to_owned(), job.id);
        }

        let id = job.id;
        inner.ready.push_back(id);
        inner.jobs.insert(
            id,
            StoredJob {
                envelope: job,
                state: JobState::Queued,
                reserved_until: None,
            },
        );
        Ok(PushResult::Created(id))
    }

    async fn reserve(&self) -> Result<Option<JobEnvelope>, QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        let now = SystemTime::now();
        // Lazily recover jobs whose visibility timeout has elapsed so any
        // caller (not just a dedicated sweeper) reclaims crashed work.
        reclaim_expired_locked(&mut inner, now);
        let len = inner.ready.len();

        for _ in 0..len {
            let Some(id) = inner.ready.pop_front() else {
                break;
            };

            let Some(stored) = inner.jobs.get_mut(&id) else {
                continue;
            };

            if stored.state != JobState::Queued {
                continue;
            }

            if stored.envelope.available_at > now {
                inner.ready.push_back(id);
                continue;
            }

            stored.state = JobState::Reserved;
            stored.reserved_until = self.visibility_timeout.map(|timeout| now + timeout);
            stored.envelope.attempts = stored.envelope.attempts.saturating_add(1);
            return Ok(Some(stored.envelope.clone()));
        }

        Ok(None)
    }

    async fn ack(&self, id: &JobId) -> Result<(), QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        let stored = inner.jobs.remove(id).ok_or(QueueError::NotFound(*id))?;
        if stored.state != JobState::Reserved {
            return Err(QueueError::InvalidState { id: *id });
        }
        if let Some(key) = stored.envelope.idempotency_key.as_deref()
            && inner.idempotency.get(key).copied() == Some(*id)
        {
            inner.idempotency.remove(key);
        }
        Ok(())
    }

    async fn fail(&self, id: &JobId, available_at: SystemTime) -> Result<(), QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        let stored = inner.jobs.get_mut(id).ok_or(QueueError::NotFound(*id))?;
        if stored.state != JobState::Reserved {
            return Err(QueueError::InvalidState { id: *id });
        }
        stored.envelope.available_at = available_at;
        stored.state = JobState::Queued;
        stored.reserved_until = None;
        inner.ready.push_back(*id);
        Ok(())
    }

    async fn dead_letter(&self, id: &JobId) -> Result<(), QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        let mut stored = inner.jobs.remove(id).ok_or(QueueError::NotFound(*id))?;
        if stored.state != JobState::Reserved {
            return Err(QueueError::InvalidState { id: *id });
        }
        if let Some(key) = stored.envelope.idempotency_key.as_deref()
            && inner.idempotency.get(key).copied() == Some(*id)
        {
            inner.idempotency.remove(key);
        }
        stored.state = JobState::Dead;
        inner.dead_letters.push(stored.envelope);
        Ok(())
    }

    async fn reclaim_expired(&self) -> Result<usize, QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        Ok(reclaim_expired_locked(&mut inner, SystemTime::now()))
    }

    async fn purge_expired_idempotency(&self) -> Result<usize, QueueError> {
        // Keys are released on terminal states; nothing time-based to purge.
        Ok(0)
    }
}

/// Return reserved jobs whose visibility deadline has passed to the ready set.
/// Reclaimed jobs keep their (already-incremented) `attempts`, so repeatedly
/// crashing on the same job still exhausts `max_attempts` and dead-letters.
fn reclaim_expired_locked(inner: &mut Inner, now: SystemTime) -> usize {
    let expired: Vec<JobId> = inner
        .jobs
        .iter()
        .filter(|(_, stored)| {
            stored.state == JobState::Reserved
                && stored
                    .reserved_until
                    .is_some_and(|deadline| deadline <= now)
        })
        .map(|(id, _)| *id)
        .collect();

    for id in &expired {
        if let Some(stored) = inner.jobs.get_mut(id) {
            stored.state = JobState::Queued;
            stored.reserved_until = None;
            inner.ready.push_back(*id);
        }
    }
    expired.len()
}
