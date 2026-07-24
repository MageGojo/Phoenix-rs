//! Worker loop: reserve → handle → ack / fail / dead-letter.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[cfg(feature = "metrics")]
use phoenix_metrics::{JobOutcome, Metrics};
use tokio::sync::watch;

use crate::{JobError, JobHandler, QueueBackend, QueueError};

/// Configuration for [`Worker`].
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Sleep duration when `reserve` returns `None`.
    pub poll_interval: Duration,
    /// Base delay for exponential backoff after a retryable failure.
    ///
    /// Delay = `base_backoff * 2^(attempts - 1)`, saturating at about 1 hour.
    pub base_backoff: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(200),
            base_backoff: Duration::from_secs(1),
        }
    }
}

impl WorkerConfig {
    /// Builder-style poll interval override.
    #[must_use]
    pub const fn poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Builder-style base backoff override.
    #[must_use]
    pub const fn base_backoff(mut self, base_backoff: Duration) -> Self {
        self.base_backoff = base_backoff;
        self
    }
}

/// Cloneable shutdown token backed by a [`watch`] channel.
#[derive(Clone, Debug)]
pub struct ShutdownToken {
    rx: watch::Receiver<bool>,
}

impl ShutdownToken {
    /// Whether shutdown has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Wait until shutdown is signalled.
    pub async fn cancelled(&mut self) {
        if self.is_cancelled() {
            return;
        }
        while self.rx.changed().await.is_ok() {
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// Producer half of a shutdown signal.
#[derive(Clone, Debug)]
pub struct ShutdownSignal {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Create a paired signal / token.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    /// Obtain a token for workers.
    #[must_use]
    pub fn token(&self) -> ShutdownToken {
        ShutdownToken {
            rx: self.rx.clone(),
        }
    }

    /// Request graceful shutdown.
    pub fn shutdown(&self) {
        let _ = self.tx.send(true);
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Background worker that drains a [`QueueBackend`].
pub struct Worker<B, H> {
    backend: Arc<B>,
    handler: H,
    config: WorkerConfig,
    #[cfg(feature = "metrics")]
    metrics: Option<Metrics>,
    shutdown: ShutdownToken,
}

impl<B, H> Worker<B, H>
where
    B: QueueBackend,
    H: JobHandler,
{
    /// Construct a worker.
    #[must_use]
    pub fn new(backend: Arc<B>, handler: H, shutdown: ShutdownToken) -> Self {
        Self {
            backend,
            handler,
            config: WorkerConfig::default(),
            #[cfg(feature = "metrics")]
            metrics: None,
            shutdown,
        }
    }

    /// Override poll / backoff settings.
    #[must_use]
    pub fn with_config(mut self, config: WorkerConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach a shared [`Metrics`] registry for Completed / Failed / Retried.
    #[cfg(feature = "metrics")]
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Run until shutdown is requested.
    ///
    /// # Errors
    ///
    /// Propagates backend errors from `reserve` / `ack` / `fail` / `dead_letter`.
    pub async fn run(mut self) -> Result<(), QueueError> {
        loop {
            if self.shutdown.is_cancelled() {
                return Ok(());
            }

            match self.backend.reserve().await? {
                Some(job) => {
                    let max_attempts = job.max_attempts;
                    let attempts = job.attempts;
                    let id = job.id;
                    let result = self.handler.handle(job).await;

                    match result {
                        Ok(()) => {
                            self.backend.ack(&id).await?;
                            self.record_completed();
                        }
                        Err(error) if should_dead_letter(&error, attempts, max_attempts) => {
                            self.backend.dead_letter(&id).await?;
                            self.record_failed();
                        }
                        Err(_) => {
                            let delay = backoff_delay(self.config.base_backoff, attempts);
                            let available_at = SystemTime::now() + delay;
                            self.backend.fail(&id, available_at).await?;
                            self.record_retried();
                        }
                    }
                }
                None => {
                    tokio::select! {
                        () = self.shutdown.cancelled() => return Ok(()),
                        () = tokio::time::sleep(self.config.poll_interval) => {}
                    }
                }
            }
        }
    }

    #[cfg(feature = "metrics")]
    fn record_completed(&self) {
        self.record(JobOutcome::Completed);
    }

    #[cfg(not(feature = "metrics"))]
    fn record_completed(&self) {}

    #[cfg(feature = "metrics")]
    fn record_failed(&self) {
        self.record(JobOutcome::Failed);
    }

    #[cfg(not(feature = "metrics"))]
    fn record_failed(&self) {}

    #[cfg(feature = "metrics")]
    fn record_retried(&self) {
        self.record(JobOutcome::Retried);
    }

    #[cfg(not(feature = "metrics"))]
    fn record_retried(&self) {}

    #[cfg(feature = "metrics")]
    fn record(&self, outcome: JobOutcome) {
        if let Some(metrics) = &self.metrics {
            metrics.record_job(outcome);
        }
    }
}

fn should_dead_letter(error: &JobError, attempts: u32, max_attempts: u32) -> bool {
    !error.is_retryable() || attempts >= max_attempts
}

/// Exponential backoff: `base * 2^(attempts - 1)`, capped at 1 hour.
#[must_use]
pub fn backoff_delay(base: Duration, attempts: u32) -> Duration {
    let attempts = attempts.max(1);
    let shift = attempts.saturating_sub(1).min(16);
    let multiplier = 1_u32 << shift;
    let capped = base.saturating_mul(multiplier);
    let max = Duration::from_hours(1);
    if capped > max { max } else { capped }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially() {
        let base = Duration::from_secs(1);
        assert_eq!(backoff_delay(base, 1), Duration::from_secs(1));
        assert_eq!(backoff_delay(base, 2), Duration::from_secs(2));
        assert_eq!(backoff_delay(base, 3), Duration::from_secs(4));
        assert_eq!(backoff_delay(base, 4), Duration::from_secs(8));
    }
}
