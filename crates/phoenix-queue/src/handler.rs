//! Job handler contract.

use std::{future::Future, pin::Pin};

use crate::{JobEnvelope, JobError};

/// Owned, `'static` future used by handlers and the worker loop.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Async handler invoked once per reserved job.
pub trait JobHandler: Send + Sync {
    /// Process `job`. Return [`Ok`] to ack, [`Err`] to fail / dead-letter.
    fn handle(&self, job: JobEnvelope) -> BoxFuture<Result<(), JobError>>;
}

impl<F, Fut> JobHandler for F
where
    F: Fn(JobEnvelope) -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), JobError>> + Send + 'static,
{
    fn handle(&self, job: JobEnvelope) -> BoxFuture<Result<(), JobError>> {
        Box::pin(self(job))
    }
}
