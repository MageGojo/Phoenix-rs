//! Overlap lock abstraction: a default in-process guard plus an injection
//! point for a distributed (Redis-backed) lock for multi-instance deployments.
//!
//! The scheduler acquires the lock named after each job before running it and
//! releases it when the run finishes (or the task is dropped / panics). With
//! the default [`InProcessLock`] this reproduces the historical per-process
//! overlap guard; with a distributed lock it also prevents two *processes* /
//! machines from running the same job at once — Laravel's
//! `withoutOverlapping` on a shared cache mutex.

use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Owned, `'static` future yielding an acquired [`LockGuard`], or `None` when
/// the lock is already held elsewhere.
pub type BoxLockFuture = Pin<Box<dyn Future<Output = Option<LockGuard>> + Send + 'static>>;

/// Held lock. Dropping it releases the underlying lock (best-effort for
/// distributed backends, which also rely on the acquisition TTL as a backstop
/// against a crashed holder).
#[must_use = "dropping the guard immediately releases the lock"]
pub struct LockGuard {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl LockGuard {
    /// Wrap a release action that runs exactly once, on drop.
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LockGuard").finish_non_exhaustive()
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// Mutual-exclusion primitive keyed by job name, used for overlap protection.
///
/// Implementations must guarantee that while one [`LockGuard`] for `name` is
/// alive, a concurrent [`try_acquire`](ScheduleLock::try_acquire) for the same
/// `name` yields `None`. `ttl` bounds how long a holder may keep the lock if it
/// never releases (e.g. a crashed process) — the in-process lock ignores it,
/// distributed locks honour it.
pub trait ScheduleLock: Send + Sync {
    /// Try to acquire the lock for `name`, holding it for at most `ttl`.
    ///
    /// Resolves to `Some(guard)` on success (guard releases on drop) or `None`
    /// when the lock is currently held.
    fn try_acquire(&self, name: String, ttl: Duration) -> BoxLockFuture;
}

/// Default per-process overlap lock backed by a shared name set.
///
/// Equivalent to the previous per-job `AtomicBool`, but keyed by job name so
/// same-named jobs share one guard. Provides no cross-process protection —
/// inject a distributed lock for multi-instance deployments.
#[derive(Clone, Default)]
pub struct InProcessLock {
    held: Arc<Mutex<HashSet<String>>>,
}

impl InProcessLock {
    /// Create an empty in-process lock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for InProcessLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InProcessLock")
            .finish_non_exhaustive()
    }
}

impl ScheduleLock for InProcessLock {
    fn try_acquire(&self, name: String, _ttl: Duration) -> BoxLockFuture {
        let held = Arc::clone(&self.held);
        Box::pin(async move {
            {
                let mut names = held.lock().expect("in-process schedule lock poisoned");
                if !names.insert(name.clone()) {
                    return None;
                }
            }
            let release_set = Arc::clone(&held);
            Some(LockGuard::new(move || {
                if let Ok(mut names) = release_set.lock() {
                    names.remove(&name);
                }
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_process_lock_is_mutually_exclusive_per_name() {
        let lock = InProcessLock::new();
        let ttl = Duration::from_mins(1);

        let first = lock
            .try_acquire("job".to_owned(), ttl)
            .await
            .expect("first acquire");
        assert!(
            lock.try_acquire("job".to_owned(), ttl).await.is_none(),
            "second acquire of the same name is blocked"
        );
        // A different name is independent.
        assert!(lock.try_acquire("other".to_owned(), ttl).await.is_some());

        drop(first);
        assert!(
            lock.try_acquire("job".to_owned(), ttl).await.is_some(),
            "dropping the guard releases the name"
        );
    }

    #[tokio::test]
    async fn guard_release_runs_once_on_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let released = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&released);
        let guard = LockGuard::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(guard);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }
}
