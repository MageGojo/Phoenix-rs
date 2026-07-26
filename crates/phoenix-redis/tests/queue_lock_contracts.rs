//! Integration contracts for the Redis queue backend and scheduler lock.
//!
//! Gated by `PHOENIX_TEST_REDIS_URL` exactly like `tests/contracts.rs`: when it
//! is unset the tests return early (counted as passing) so the suite stays
//! green offline. The atomic Lua logic and serialization have offline unit
//! tests in `src/queue.rs` / `src/keys.rs`.
#![cfg(all(feature = "queue", feature = "schedule"))]

use std::time::Duration;

use phoenix_queue::{JobEnvelope, PushOptions, PushResult, Queue, QueueBackend};
use phoenix_redis::RedisStores;
use phoenix_schedule::ScheduleLock;

fn redis_url() -> Option<String> {
    std::env::var("PHOENIX_TEST_REDIS_URL")
        .ok()
        .filter(|value| !value.is_empty())
}

async fn stores() -> Option<RedisStores> {
    let url = redis_url()?;
    match RedisStores::connect(&url).await {
        Ok(stores) => Some(stores),
        Err(error) => {
            eprintln!("skipping redis integration: {error}");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )
}

#[tokio::test]
async fn persistent_push_reserve_ack() {
    let Some(stores) = stores().await else {
        return;
    };
    let backend = stores.queue(unique("q-basic"));
    let queue = Queue::new(std::sync::Arc::new(backend.clone()));

    let pushed = queue
        .dispatch("welcome", serde_json::json!({"user": 7}))
        .await
        .expect("push");
    assert!(pushed.is_created());

    let job = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(job.name, "welcome");
    assert_eq!(job.attempts, 1);
    assert_eq!(job.payload["user"], 7);

    // A second reserve sees nothing — the job is in-flight (invisible).
    assert!(backend.reserve().await.expect("reserve").is_none());

    backend.ack(&job.id).await.expect("ack");
    assert!(backend.reserve().await.expect("reserve").is_none());
}

#[tokio::test]
async fn delayed_job_is_invisible_until_available() {
    let Some(stores) = stores().await else {
        return;
    };
    let backend = stores.queue(unique("q-delay"));
    backend
        .push(
            JobEnvelope::new("later", serde_json::json!({}), 3, None)
                .with_delay(Duration::from_secs(2)),
        )
        .await
        .expect("push delayed");
    backend
        .push(JobEnvelope::new("now", serde_json::json!({}), 3, None))
        .await
        .expect("push immediate");

    // The runnable job is served; the delayed one is skipped, not blocking.
    let job = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(job.name, "now");
    backend.ack(&job.id).await.expect("ack");
    assert!(backend.reserve().await.expect("reserve").is_none());

    // After the delay elapses the delayed job becomes reservable.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let job = backend.reserve().await.expect("reserve").expect("due job");
    assert_eq!(job.name, "later");
    backend.ack(&job.id).await.expect("ack");
}

#[tokio::test]
async fn nack_retries_then_dead_letters() {
    let Some(stores) = stores().await else {
        return;
    };
    let backend = stores.queue(unique("q-retry"));
    let queue = Queue::new(std::sync::Arc::new(backend.clone()));
    queue
        .push_json(
            "flaky",
            serde_json::json!({}),
            PushOptions::new().max_attempts(2),
        )
        .await
        .expect("push");

    let first = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(first.attempts, 1);
    backend
        .fail(&first.id, std::time::SystemTime::now())
        .await
        .expect("nack");

    let second = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(second.attempts, 2);
    assert_eq!(second.id, first.id);
    backend.dead_letter(&second.id).await.expect("dead letter");

    assert!(backend.reserve().await.expect("reserve").is_none());
    let dead = backend.dead_letters().await.expect("dead letters");
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].id, first.id);
    assert_eq!(dead[0].attempts, 2);
}

#[tokio::test]
async fn visibility_timeout_redelivers_stuck_job() {
    let Some(stores) = stores().await else {
        return;
    };
    let backend = stores
        .queue(unique("q-visibility"))
        .with_visibility_timeout(Duration::from_secs(1));
    backend
        .push(JobEnvelope::new("stuck", serde_json::json!({}), 5, None))
        .await
        .expect("push");

    // Reserve but never resolve — the worker "crashes".
    let first = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(first.attempts, 1);
    assert!(
        backend.reserve().await.expect("reserve").is_none(),
        "invisible while reserved"
    );

    // Past the visibility deadline it is reclaimed and served again.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(backend.reclaim_expired().await.expect("reclaim"), 1);
    let second = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(second.id, first.id);
    assert_eq!(second.attempts, 2);
    backend.ack(&second.id).await.expect("ack");
}

#[tokio::test]
async fn idempotency_key_dedupes_while_in_flight() {
    let Some(stores) = stores().await else {
        return;
    };
    let backend = stores.queue(unique("q-idem"));
    let queue = Queue::new(std::sync::Arc::new(backend.clone()));

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

    let job = backend.reserve().await.expect("reserve").expect("job");
    assert_eq!(
        job.payload["a"], 1,
        "payload not replaced by duplicate push"
    );
    backend.ack(&job.id).await.expect("ack");

    // Key freed after ack — a new push creates a fresh job.
    let third = queue
        .dispatch_once("once", serde_json::json!({"a": 3}), "key-1")
        .await
        .expect("third");
    assert!(third.is_created());
    assert_ne!(third.job_id(), first.job_id());
}

#[tokio::test]
async fn two_instances_reserve_each_job_once() {
    let Some(stores) = stores().await else {
        return;
    };
    let name = unique("q-shared");
    let worker_a = stores.queue(name.clone());
    let worker_b = stores.queue(name);
    let queue = Queue::new(std::sync::Arc::new(worker_a.clone()));
    for n in 0..6 {
        queue
            .dispatch("job", serde_json::json!({ "n": n }))
            .await
            .expect("push");
    }

    let mut seen = std::collections::HashSet::new();
    for _ in 0..6 {
        let backend: &phoenix_redis::RedisQueue = if seen.len() % 2 == 0 {
            &worker_a
        } else {
            &worker_b
        };
        let job = backend.reserve().await.expect("reserve").expect("job");
        assert!(
            seen.insert(job.payload["n"].as_i64().expect("n")),
            "no double reserve"
        );
        backend.ack(&job.id).await.expect("ack");
    }
    assert_eq!(seen.len(), 6);
    assert!(worker_a.reserve().await.expect("reserve").is_none());
}

#[tokio::test]
async fn schedule_lock_is_mutually_exclusive() {
    let Some(stores) = stores().await else {
        return;
    };
    let lock = stores.schedule_lock();
    let name = unique("lock-mutex");
    let ttl = Duration::from_mins(1);

    let held = lock
        .try_acquire(name.clone(), ttl)
        .await
        .expect("first acquire");
    // Another instance sharing the same Redis cannot acquire it.
    let other = stores.schedule_lock();
    assert!(other.try_acquire(name.clone(), ttl).await.is_none());

    drop(held);
    // Release runs on drop (spawned); give it a moment, then it is free again.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        other.try_acquire(name, ttl).await.is_some(),
        "lock released on guard drop"
    );
}

#[tokio::test]
async fn schedule_lock_expires_and_release_never_steals() {
    let Some(stores) = stores().await else {
        return;
    };
    let lock = stores.schedule_lock();
    let name = unique("lock-ttl");

    // Short TTL; hold the guard so only the PX expiry can free it.
    let first = lock
        .try_acquire(name.clone(), Duration::from_secs(1))
        .await
        .expect("first acquire");
    assert!(
        lock.try_acquire(name.clone(), Duration::from_secs(1))
            .await
            .is_none()
    );

    tokio::time::sleep(Duration::from_millis(1300)).await;
    // After the TTL lapses a second holder acquires it.
    let second = lock
        .try_acquire(name.clone(), Duration::from_mins(1))
        .await
        .expect("re-acquire after expiry");

    // Dropping the first (expired) guard must NOT delete the second holder's
    // lock — the release is owner-guarded by token.
    drop(first);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        lock.try_acquire(name, Duration::from_secs(1))
            .await
            .is_none(),
        "second holder still owns the lock after the first guard drops"
    );
    drop(second);
}
