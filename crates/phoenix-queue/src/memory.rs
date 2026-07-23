//! In-process queue backend.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::SystemTime,
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
#[derive(Default)]
pub struct MemoryQueue {
    inner: Mutex<Inner>,
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
            },
        );
        Ok(PushResult::Created(id))
    }

    async fn reserve(&self) -> Result<Option<JobEnvelope>, QueueError> {
        let mut inner = self.inner.lock().expect("memory queue poisoned");
        let now = SystemTime::now();
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

    async fn purge_expired_idempotency(&self) -> Result<usize, QueueError> {
        // Keys are released on terminal states; nothing time-based to purge.
        Ok(0)
    }
}
