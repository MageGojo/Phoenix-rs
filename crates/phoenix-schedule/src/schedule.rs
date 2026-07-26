//! Schedule DSL and the two runners (`run_due` one-shot, `work` loop).

use std::{
    error::Error,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime},
};

use phoenix_queue::ShutdownToken;

use crate::{
    lock::{InProcessLock, ScheduleLock},
    spec::{Spec, next_run_unix, next_run_with_offset, system_time_from_unix, unix_secs},
};

/// Default overlap-lock TTL: how long a distributed lock is held if the holder
/// crashes without releasing. Generous so it exceeds normal task durations; a
/// task that runs longer than this risks a second instance starting it.
pub const DEFAULT_LOCK_TTL: Duration = Duration::from_hours(1);

/// Result type returned by scheduled tasks.
pub type TaskResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Owned, `'static` future produced by a scheduled task.
pub type BoxTaskFuture = Pin<Box<dyn Future<Output = TaskResult> + Send + 'static>>;

/// Value a scheduled task future may resolve to.
///
/// Implemented for `()` (always success) and `Result<(), E>`, so closures may
/// return either.
pub trait TaskOutcome {
    /// Normalize into a [`TaskResult`].
    ///
    /// # Errors
    ///
    /// Forwards the task's own error, if any; `()` never fails.
    fn into_task_result(self) -> TaskResult;
}

impl TaskOutcome for () {
    fn into_task_result(self) -> TaskResult {
        Ok(())
    }
}

impl<E> TaskOutcome for Result<(), E>
where
    E: Into<Box<dyn Error + Send + Sync>>,
{
    fn into_task_result(self) -> TaskResult {
        self.map_err(Into::into)
    }
}

/// Async task invoked whenever its spec fires.
pub trait ScheduledTask: Send + Sync {
    /// Start one execution of the task.
    fn run(&self) -> BoxTaskFuture;
}

impl<F, Fut, Out> ScheduledTask for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Out> + Send + 'static,
    Out: TaskOutcome,
{
    fn run(&self) -> BoxTaskFuture {
        let future = self();
        Box::pin(async move { future.await.into_task_result() })
    }
}

/// One registered job.
struct Job {
    name: String,
    spec: Spec,
    task: Arc<dyn ScheduledTask>,
}

/// Outcome of one spawned job attempt.
enum JobRun {
    /// Ran and returned `Ok`.
    Completed,
    /// Ran and returned `Err` (or panicked).
    Failed,
    /// Skipped: the overlap lock was already held (previous run still in
    /// flight, or another instance holds a distributed lock).
    Skipped,
}

/// Counters produced by one scheduler round.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunSummary {
    /// Jobs whose spec fired in this round.
    pub due: usize,
    /// Jobs that ran and returned `Ok`.
    pub completed: usize,
    /// Jobs that ran and returned `Err` (or panicked).
    pub failed: usize,
    /// Jobs skipped because their previous execution was still running.
    pub skipped: usize,
}

/// Laravel-style task schedule.
///
/// ```
/// use phoenix_schedule::{Schedule, cron, every_minutes};
///
/// let schedule = Schedule::new()
///     .job("sitemap", every_minutes(30), || async {
///         // regenerate the sitemap …
///     })
///     .job("nightly-report", cron("0 3 * * *"), || async {
///         Ok::<(), std::io::Error>(())
///     });
/// assert_eq!(schedule.len(), 2);
/// ```
pub struct Schedule {
    jobs: Vec<Job>,
    utc_offset_secs: i32,
    lock: Arc<dyn ScheduleLock>,
    lock_ttl: Duration,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            utc_offset_secs: 0,
            lock: Arc::new(InProcessLock::new()),
            lock_ttl: DEFAULT_LOCK_TTL,
        }
    }
}

impl Schedule {
    /// Create an empty schedule (wall-clock specs interpreted in UTC).
    ///
    /// Overlap protection defaults to an in-process lock; inject a distributed
    /// one with [`with_lock`](Self::with_lock) for multi-instance deployments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the overlap lock (default: [`InProcessLock`]).
    ///
    /// Inject a distributed lock (e.g. `phoenix_redis::RedisScheduleLock`) so
    /// that only one instance across the whole fleet runs a given job at once.
    /// The lock is keyed by job name.
    #[must_use]
    pub fn with_lock(mut self, lock: Arc<dyn ScheduleLock>) -> Self {
        self.lock = lock;
        self
    }

    /// Override the overlap-lock TTL (default [`DEFAULT_LOCK_TTL`]).
    ///
    /// Only meaningful for distributed locks: it bounds how long the lock
    /// survives a crashed holder. Set it above the job's longest expected
    /// runtime to avoid a second instance starting the job mid-run.
    #[must_use]
    pub const fn lock_ttl(mut self, ttl: Duration) -> Self {
        self.lock_ttl = ttl;
        self
    }

    /// Interpret wall-clock specs (`daily_at`, cron) at a fixed offset east
    /// of UTC, in seconds (e.g. `8 * 3600` for UTC+8).
    #[must_use]
    pub const fn utc_offset_secs(mut self, secs: i32) -> Self {
        self.utc_offset_secs = secs;
        self
    }

    /// Convenience wrapper for [`Self::utc_offset_secs`] in whole hours.
    #[must_use]
    pub fn utc_offset_hours(self, hours: i8) -> Self {
        self.utc_offset_secs(i32::from(hours) * 3600)
    }

    /// Register a job. `name` is used for logging and the overlap guard.
    #[must_use]
    pub fn job(
        mut self,
        name: impl Into<String>,
        spec: Spec,
        task: impl ScheduledTask + 'static,
    ) -> Self {
        self.jobs.push(Job {
            name: name.into(),
            spec,
            task: Arc::new(task),
        });
        self
    }

    /// Number of registered jobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether no jobs are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// `(name, next_run)` for every job, strictly after `after`.
    ///
    /// `None` marks unreachable cron expressions.
    #[must_use]
    pub fn next_runs(&self, after: SystemTime) -> Vec<(&str, Option<SystemTime>)> {
        self.jobs
            .iter()
            .map(|job| {
                (
                    job.name.as_str(),
                    next_run_with_offset(after, &job.spec, self.utc_offset_secs),
                )
            })
            .collect()
    }

    /// Run one round: execute every job due in the minute containing `now`,
    /// wait for them to finish, and return counters. Designed for
    /// `schedule:run` under an external crontab that fires every minute.
    ///
    /// A job is *due* when its next occurrence (computed from the start of
    /// the minute, inclusive) falls inside that minute. Sub-minute interval
    /// jobs therefore run **once** per `run_due` round; use
    /// [`Self::work`] for exact sub-minute pacing.
    ///
    /// Failures are logged via `tracing` and never abort other jobs.
    pub async fn run_due(&self, now: SystemTime) -> RunSummary {
        let now = unix_secs(now);
        let window_start = now.div_euclid(60) * 60;
        let window_end = window_start + 60;

        let mut summary = RunSummary::default();
        let mut handles = Vec::new();
        for job in &self.jobs {
            let due = next_run_unix(window_start - 1, &job.spec, self.utc_offset_secs)
                .is_some_and(|next| next < window_end);
            if !due {
                continue;
            }
            summary.due += 1;
            handles.push(spawn_job(job, Arc::clone(&self.lock), self.lock_ttl));
        }

        for handle in handles {
            match handle.await {
                Ok(JobRun::Completed) => summary.completed += 1,
                Ok(JobRun::Skipped) => summary.skipped += 1,
                Ok(JobRun::Failed) | Err(_) => summary.failed += 1,
            }
        }
        summary
    }

    /// Long-running loop: sleep until the next due job (checking at least
    /// once per minute), fire due jobs, and repeat until `shutdown` is
    /// signalled. In-flight jobs are awaited before returning.
    ///
    /// Reuses the `phoenix-queue` [`phoenix_queue::ShutdownSignal`] /
    /// [`ShutdownToken`] pair for graceful shutdown.
    pub async fn work(&self, mut shutdown: ShutdownToken) {
        let now = unix_secs(SystemTime::now());
        let mut deadlines: Vec<Option<i64>> = self
            .jobs
            .iter()
            .map(|job| next_run_unix(now, &job.spec, self.utc_offset_secs))
            .collect();
        let mut inflight: Vec<tokio::task::JoinHandle<JobRun>> = Vec::new();

        while !shutdown.is_cancelled() {
            let now = unix_secs(SystemTime::now());

            for (job, deadline) in self.jobs.iter().zip(deadlines.iter_mut()) {
                let Some(due_at) = *deadline else { continue };
                if due_at > now {
                    continue;
                }
                // Overlap is enforced inside the spawned task by the lock; a
                // job whose previous run still holds it resolves to `Skipped`.
                inflight.push(spawn_job(job, Arc::clone(&self.lock), self.lock_ttl));
                *deadline = next_run_unix(now, &job.spec, self.utc_offset_secs);
            }
            inflight.retain(|handle| !handle.is_finished());

            // Sleep until the earliest deadline, but never more than a
            // minute so the loop stays responsive to wall-clock changes.
            let now = unix_secs(SystemTime::now());
            let sleep_secs = deadlines
                .iter()
                .flatten()
                .map(|deadline| (deadline - now).max(0))
                .min()
                .unwrap_or(60)
                .min(60);
            let sleep_for =
                Duration::from_secs(sleep_secs.unsigned_abs()).max(Duration::from_millis(250));
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(sleep_for) => {}
            }
        }

        for handle in inflight {
            let _ = handle.await;
        }
    }

    /// The instant a job would consider "due" next; exposed for tests.
    #[must_use]
    pub fn next_run_of(&self, name: &str, after: SystemTime) -> Option<SystemTime> {
        self.jobs
            .iter()
            .find(|job| job.name == name)
            .and_then(|job| {
                next_run_unix(unix_secs(after), &job.spec, self.utc_offset_secs)
                    .map(system_time_from_unix)
            })
    }
}

/// Spawn `job` on the Tokio runtime, gated by the overlap `lock`.
///
/// The spawned task first tries to acquire the lock named after the job. If it
/// is already held (previous run still in flight, or another instance owns a
/// distributed lock) the task resolves to [`JobRun::Skipped`] without running.
/// Otherwise it runs the task and releases the lock on completion — the guard
/// also releases if the future is dropped mid-flight (shutdown) or panics.
fn spawn_job(
    job: &Job,
    lock: Arc<dyn ScheduleLock>,
    ttl: Duration,
) -> tokio::task::JoinHandle<JobRun> {
    let task = Arc::clone(&job.task);
    let name = job.name.clone();
    tokio::spawn(async move {
        let Some(_guard) = lock.try_acquire(name.clone(), ttl).await else {
            tracing::warn!(job = %name, "skipped: overlap lock held (previous run or another instance)");
            return JobRun::Skipped;
        };
        tracing::debug!(job = %name, "scheduled job started");
        match task.run().await {
            Ok(()) => {
                tracing::debug!(job = %name, "scheduled job completed");
                JobRun::Completed
            }
            Err(error) => {
                tracing::error!(job = %name, error = %error, "scheduled job failed");
                JobRun::Failed
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use phoenix_queue::ShutdownSignal;

    use super::*;
    use crate::spec::{cron, every_seconds};

    fn at_unix(secs: i64) -> SystemTime {
        system_time_from_unix(secs)
    }

    #[tokio::test]
    async fn run_due_runs_only_due_jobs() {
        let ran_every = Arc::new(AtomicUsize::new(0));
        let ran_daily = Arc::new(AtomicUsize::new(0));
        let every_counter = Arc::clone(&ran_every);
        let daily_counter = Arc::clone(&ran_daily);

        let schedule = Schedule::new()
            .job("every-minute", every_seconds(60), move || {
                let counter = Arc::clone(&every_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            })
            .job("at-03", cron("0 3 * * *"), move || {
                let counter = Arc::clone(&daily_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            });

        // 2026-07-26 10:00:30 UTC — only the minute job is due.
        let now = at_unix(1_785_024_000 + 10 * 3600 + 30);
        let summary = schedule.run_due(now).await;
        assert_eq!(summary.due, 1);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.skipped, 0);
        assert_eq!(ran_every.load(Ordering::SeqCst), 1);
        assert_eq!(ran_daily.load(Ordering::SeqCst), 0);

        // 03:00:05 — both are due.
        let three_am = at_unix(1_785_024_000 + 3 * 3600 + 5);
        let summary = schedule.run_due(three_am).await;
        assert_eq!(summary.due, 2);
        assert_eq!(summary.completed, 2);
        assert_eq!(ran_daily.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_due_counts_failures_without_aborting_others() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let schedule = Schedule::new()
            .job("boom", every_seconds(60), || async {
                Err::<(), _>(std::io::Error::other("boom"))
            })
            .job("fine", every_seconds(60), move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            });

        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.due, 2);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_due_counts_panics_as_failures() {
        fn kaboom() -> TaskResult {
            panic!("kaboom")
        }
        let schedule = Schedule::new().job("panics", every_seconds(60), || async { kaboom() });
        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.due, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.completed, 0);
    }

    #[tokio::test]
    async fn overlapping_job_is_skipped() {
        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));

        let schedule = Arc::new(Schedule::new().job("slow", every_seconds(1), move || {
            let counter = Arc::clone(&counter);
            let release_rx = Arc::clone(&release_rx);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                if let Some(receiver) = release_rx.lock().await.take() {
                    let _ = receiver.await;
                }
            }
        }));

        // First round: job starts and blocks on the oneshot.
        let first = Arc::clone(&schedule);
        let first_round = tokio::spawn(async move { first.run_due(SystemTime::now()).await });
        // Wait until the slow job is definitely started.
        while started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Second round while the first execution is still running → skipped.
        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.due, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.completed, 0);
        assert_eq!(started.load(Ordering::SeqCst), 1, "no second start");

        let _ = release_tx.send(());
        let first_summary = first_round.await.expect("first round");
        assert_eq!(first_summary.completed, 1);

        // After completion the guard is released and the job can run again.
        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.completed, 1);
    }

    #[tokio::test]
    async fn work_executes_due_jobs_and_stops_on_shutdown() {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let schedule = Arc::new(Schedule::new().job("tick", every_seconds(1), move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let signal = ShutdownSignal::new();
        let token = signal.token();
        let runner = Arc::clone(&schedule);
        let worker = tokio::spawn(async move { runner.work(token).await });

        // every_seconds(1) must fire within ~1.5s.
        let mut waited = Duration::ZERO;
        while ran.load(Ordering::SeqCst) == 0 && waited < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            waited += Duration::from_millis(50);
        }
        assert!(ran.load(Ordering::SeqCst) >= 1, "job fired in work loop");

        signal.shutdown();
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .expect("work loop exits promptly on shutdown")
            .expect("join");
    }

    #[tokio::test]
    async fn work_returns_immediately_when_already_shut_down() {
        let schedule = Schedule::new();
        let signal = ShutdownSignal::new();
        signal.shutdown();
        tokio::time::timeout(Duration::from_secs(1), schedule.work(signal.token()))
            .await
            .expect("pre-cancelled token exits immediately");
    }

    #[tokio::test]
    async fn injected_lock_can_force_skip() {
        use std::time::Duration;

        use crate::lock::{BoxLockFuture, ScheduleLock};

        // A lock that never grants acquisition — every job is skipped.
        struct NeverLock;
        impl ScheduleLock for NeverLock {
            fn try_acquire(&self, _name: String, _ttl: Duration) -> BoxLockFuture {
                Box::pin(async { None })
            }
        }

        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let schedule = Schedule::new().with_lock(Arc::new(NeverLock)).job(
            "tick",
            every_seconds(60),
            move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            },
        );

        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.due, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.completed, 0);
        assert_eq!(ran.load(Ordering::SeqCst), 0, "task never ran");
    }

    #[tokio::test]
    async fn injected_lock_that_grants_runs_the_job() {
        use crate::lock::InProcessLock;

        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let schedule = Schedule::new()
            .with_lock(Arc::new(InProcessLock::new()))
            .job("tick", every_seconds(60), move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            });

        let summary = schedule.run_due(SystemTime::now()).await;
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn next_runs_reports_every_job() {
        let schedule = Schedule::new()
            .job("a", every_seconds(30), || async {})
            .job("unreachable", cron("0 0 30 2 *"), || async {});
        let now = SystemTime::now();
        let runs = schedule.next_runs(now);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, "a");
        assert!(runs[0].1.is_some());
        assert_eq!(runs[1], ("unreachable", None));
        assert!(schedule.next_run_of("a", now).expect("some") > now);
        assert!(schedule.next_run_of("missing", now).is_none());
    }

    #[test]
    fn utc_offset_shifts_wall_clock_specs() {
        let schedule = Schedule::new().utc_offset_hours(8).job(
            "daily",
            crate::spec::daily_at("03:00"),
            || async {},
        );
        // 2026-07-26 12:00 UTC → next 03:00 UTC+8 is 19:00 UTC.
        let after = at_unix(1_785_024_000 + 12 * 3600);
        let next = schedule.next_run_of("daily", after).expect("some");
        assert_eq!(unix_secs(next), 1_785_024_000 + 19 * 3600);
    }
}
