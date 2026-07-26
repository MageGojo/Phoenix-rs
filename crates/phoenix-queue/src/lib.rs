//! Background job queue for Phoenix.
//!
//! Provides a [`QueueBackend`] contract, an in-process [`MemoryQueue`], a
//! [`Queue`] facade for JSON dispatch, and a [`Worker`] loop with exponential
//! backoff, idempotency, dead-lettering, and graceful shutdown.
//!
//! See `docs/QUEUE.md` and `docs/QUEUE_MAIL_CONSOLE.md`.

#![forbid(unsafe_code)]

mod backend;
mod error;
mod handler;
mod job;
mod memory;
mod queue;
mod worker;

pub use backend::{PushResult, QueueBackend};
pub use error::{JobError, QueueError};
pub use handler::{BoxFuture, JobHandler};
pub use job::{JobEnvelope, JobId};
pub use memory::MemoryQueue;
pub use queue::{DEFAULT_MAX_ATTEMPTS, PushOptions, Queue};
pub use worker::{ShutdownSignal, ShutdownToken, Worker, WorkerConfig, backoff_delay};

#[must_use]
pub const fn crate_name() -> &'static str {
    "phoenix-queue"
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime},
    };

    use phoenix_metrics::Metrics;
    use tokio::sync::oneshot;

    use super::*;

    fn queue_counter(rendered: &str, outcome: &str) -> u64 {
        let needle = format!("phoenix_queue_jobs_total{{outcome=\"{outcome}\"}} ");
        rendered
            .lines()
            .find_map(|line| line.strip_prefix(&needle)?.parse().ok())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn push_reserve_ack() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));

        let result = queue
            .push_json("demo", serde_json::json!({"n": 1}), PushOptions::default())
            .await
            .expect("push");
        assert!(result.is_created());

        let job = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(job.name, "demo");
        assert_eq!(job.attempts, 1);
        assert_eq!(job.payload["n"], 1);

        backend.ack(&job.id).await.expect("ack");
        assert!(backend.is_empty());
        assert!(backend.reserve().await.expect("empty").is_none());
    }

    #[tokio::test]
    async fn delayed_job_is_not_reserved_until_available() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));

        queue
            .dispatch_in(
                "later",
                serde_json::json!({"n": 1}),
                Duration::from_millis(80),
            )
            .await
            .expect("push");

        assert!(
            backend.reserve().await.expect("reserve").is_none(),
            "delayed job must stay invisible before its delay elapses"
        );
        assert_eq!(backend.len(), 1, "job is queued, just not runnable yet");

        tokio::time::sleep(Duration::from_millis(120)).await;
        let job = backend.reserve().await.expect("reserve").expect("job due");
        assert_eq!(job.name, "later");
        backend.ack(&job.id).await.expect("ack");
    }

    #[tokio::test]
    async fn delayed_job_does_not_block_runnable_jobs() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));

        queue
            .push_json(
                "slow-lane",
                serde_json::json!({}),
                PushOptions::new().delay(Duration::from_mins(1)),
            )
            .await
            .expect("push delayed");
        queue
            .dispatch("fast-lane", serde_json::json!({}))
            .await
            .expect("push immediate");

        let job = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(
            job.name, "fast-lane",
            "delayed job is skipped, not blocking"
        );
        backend.ack(&job.id).await.expect("ack");
        assert!(backend.reserve().await.expect("reserve").is_none());
        assert_eq!(backend.len(), 1, "delayed job still pending");
    }

    #[tokio::test]
    async fn worker_runs_delayed_job_only_after_delay() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));
        let (done_tx, done_rx) = oneshot::channel::<SystemTime>();
        let done_tx = Arc::new(tokio::sync::Mutex::new(Some(done_tx)));

        let delay = Duration::from_millis(60);
        let enqueued_at = SystemTime::now();
        queue
            .dispatch_in("delayed-work", serde_json::json!({}), delay)
            .await
            .expect("push");

        let signal = ShutdownSignal::new();
        let shutdown = signal.token();
        let done = Arc::clone(&done_tx);
        let worker = Worker::new(
            Arc::clone(&backend),
            move |_job: JobEnvelope| {
                let done = Arc::clone(&done);
                async move {
                    if let Some(sender) = done.lock().await.take() {
                        let _ = sender.send(SystemTime::now());
                    }
                    Ok(())
                }
            },
            shutdown,
        )
        .with_config(WorkerConfig::default().poll_interval(Duration::from_millis(5)));

        let join = tokio::spawn(async move { worker.run().await });
        let handled_at = done_rx.await.expect("handler fired");
        signal.shutdown();
        join.await.expect("join").expect("worker ok");

        let elapsed = handled_at
            .duration_since(enqueued_at)
            .expect("handled after enqueue");
        assert!(
            elapsed >= delay,
            "job ran after {elapsed:?}, before its {delay:?} delay elapsed"
        );
        assert!(backend.is_empty());
    }

    #[tokio::test]
    async fn failed_retries_then_dead_letter() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));

        queue
            .push_json(
                "flaky",
                serde_json::json!({}),
                PushOptions::new().max_attempts(2),
            )
            .await
            .expect("push");

        let first = backend.reserve().await.expect("r1").expect("job");
        assert_eq!(first.attempts, 1);
        backend
            .fail(&first.id, SystemTime::now())
            .await
            .expect("fail1");

        let second = backend.reserve().await.expect("r2").expect("job");
        assert_eq!(second.attempts, 2);
        assert_eq!(second.id, first.id);
        backend.dead_letter(&second.id).await.expect("dl");

        assert!(backend.reserve().await.expect("none").is_none());
        let letters = backend.dead_letters();
        assert_eq!(letters.len(), 1);
        assert_eq!(letters[0].id, first.id);
        assert_eq!(letters[0].attempts, 2);
    }

    #[tokio::test]
    async fn idempotency_key_returns_existing_while_in_flight() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));

        let first = queue
            .dispatch_once("once", serde_json::json!({"a": 1}), "key-1")
            .await
            .expect("first");
        assert!(matches!(first, PushResult::Created(_)));

        let second = queue
            .dispatch_once("once", serde_json::json!({"a": 2}), "key-1")
            .await
            .expect("second");
        assert_eq!(second, PushResult::Existing(first.job_id()));
        assert_eq!(backend.len(), 1);

        let job = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(job.payload["a"], 1);

        let third = queue
            .dispatch_once("once", serde_json::json!({"a": 3}), "key-1")
            .await
            .expect("third while reserved");
        assert_eq!(third, PushResult::Existing(first.job_id()));

        backend.ack(&job.id).await.expect("ack");

        let fourth = queue
            .dispatch_once("once", serde_json::json!({"a": 4}), "key-1")
            .await
            .expect("after ack");
        assert!(matches!(fourth, PushResult::Created(_)));
        assert_ne!(fourth.job_id(), first.job_id());
    }

    #[tokio::test]
    async fn visibility_timeout_reclaims_stuck_reservation() {
        let backend = MemoryQueue::new().with_visibility_timeout(Duration::from_millis(40));
        let queue = Queue::new(Arc::new(backend));
        queue
            .dispatch("stuck", serde_json::json!({}))
            .await
            .expect("push");
        let backend = queue.backend();

        // Reserve but never ack/fail — simulates a crashed worker.
        let first = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(first.attempts, 1);
        assert!(
            backend.reserve().await.expect("reserve").is_none(),
            "job is invisible while reserved"
        );

        tokio::time::sleep(Duration::from_millis(70)).await;

        // Eager reclaim returns it to the ready set, then it can be reserved again.
        assert_eq!(backend.reclaim_expired().await.expect("reclaim"), 1);
        let second = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(second.id, first.id, "same job redelivered");
        assert_eq!(second.attempts, 2, "each reservation counts an attempt");
        backend.ack(&second.id).await.expect("ack");
        assert_eq!(backend.reclaim_expired().await.expect("reclaim"), 0);
    }

    #[tokio::test]
    async fn reserve_lazily_reclaims_expired_reservation() {
        let backend =
            Arc::new(MemoryQueue::new().with_visibility_timeout(Duration::from_millis(30)));
        backend
            .push(JobEnvelope::new("v", serde_json::json!({}), 5, None))
            .await
            .expect("push");

        let first = backend.reserve().await.expect("reserve").expect("job");
        tokio::time::sleep(Duration::from_millis(55)).await;
        // No explicit reclaim: reserve() itself recovers the stuck job.
        let second = backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(second.id, first.id);
        assert_eq!(second.attempts, 2);
    }

    #[tokio::test]
    async fn without_visibility_timeout_reserved_stays_reserved() {
        let backend = Arc::new(MemoryQueue::new());
        backend
            .push(JobEnvelope::new("keep", serde_json::json!({}), 5, None))
            .await
            .expect("push");
        backend.reserve().await.expect("reserve").expect("job");
        assert_eq!(backend.reclaim_expired().await.expect("reclaim"), 0);
        assert!(backend.reserve().await.expect("reserve").is_none());
    }

    #[tokio::test]
    async fn worker_handles_one_job_then_shutdown() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));
        let metrics = Metrics::new();
        let handled = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let done_tx = Arc::new(tokio::sync::Mutex::new(Some(done_tx)));

        queue
            .dispatch("work", serde_json::json!({"ok": true}))
            .await
            .expect("push");

        let signal = ShutdownSignal::new();
        let shutdown = signal.token();
        let counter = Arc::clone(&handled);
        let done = Arc::clone(&done_tx);

        let worker = Worker::new(
            Arc::clone(&backend),
            move |job: JobEnvelope| {
                let counter = Arc::clone(&counter);
                let done = Arc::clone(&done);
                async move {
                    assert_eq!(job.name, "work");
                    counter.fetch_add(1, Ordering::SeqCst);
                    if let Some(tx) = done.lock().await.take() {
                        let _ = tx.send(());
                    }
                    Ok(())
                }
            },
            shutdown,
        )
        .with_config(WorkerConfig::default().poll_interval(Duration::from_millis(10)))
        .with_metrics(metrics.clone());

        let join = tokio::spawn(async move { worker.run().await });

        done_rx.await.expect("handler fired");
        signal.shutdown();
        join.await.expect("join").expect("worker ok");

        assert_eq!(handled.load(Ordering::SeqCst), 1);
        assert!(backend.is_empty());
        let rendered = metrics.render();
        assert_eq!(queue_counter(&rendered, "completed"), 1);
        assert_eq!(queue_counter(&rendered, "failed"), 0);
        assert_eq!(queue_counter(&rendered, "retried"), 0);
    }

    #[tokio::test]
    async fn worker_retries_then_dead_letters_with_metrics() {
        let backend = Arc::new(MemoryQueue::new());
        let queue = Queue::new(Arc::clone(&backend));
        let metrics = Metrics::new();
        let attempts = Arc::new(AtomicUsize::new(0));

        queue
            .push_json(
                "always-fail",
                serde_json::json!({}),
                PushOptions::new().max_attempts(2),
            )
            .await
            .expect("push");

        let signal = ShutdownSignal::new();
        let shutdown = signal.token();
        let counter = Arc::clone(&attempts);

        let worker = Worker::new(
            Arc::clone(&backend),
            move |_job: JobEnvelope| {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(JobError::retryable("boom"))
                }
            },
            shutdown,
        )
        .with_config(
            WorkerConfig::default()
                .poll_interval(Duration::from_millis(5))
                .base_backoff(Duration::from_millis(1)),
        )
        .with_metrics(metrics.clone());

        let join = tokio::spawn(async move { worker.run().await });

        // Wait until dead-lettered.
        for _ in 0..200 {
            if !backend.dead_letters().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        signal.shutdown();
        join.await.expect("join").expect("worker ok");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(backend.dead_letters().len(), 1);
        let rendered = metrics.render();
        assert_eq!(queue_counter(&rendered, "retried"), 1);
        assert_eq!(queue_counter(&rendered, "failed"), 1);
        assert_eq!(queue_counter(&rendered, "completed"), 0);
    }
}
