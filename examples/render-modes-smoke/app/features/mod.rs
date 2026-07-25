mod routes;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use phoenix::plugin::{Capability, FeatureError, FeatureSet};
use phoenix::prelude::{
    AuthorizationEngine, JwtConfig, JwtKey, JwtManager, LocalDisk, Mailer, MemoryQueue,
    MemoryTransport, Permission, Queue, Rbac, Role, ShutdownSignal, Worker,
};

use crate::plugins::GreeterPlugin;

pub use routes::http_routes;

/// Shared handles for feature smoke endpoints.
#[derive(Clone)]
pub struct FeatureServices {
    pub jwt: Arc<JwtManager>,
    pub authorizer: Arc<AuthorizationEngine<()>>,
    pub storage: LocalDisk,
    pub queue: Arc<Queue<MemoryQueue>>,
    pub mailer: Mailer,
    pub mail_sent: MemoryTransport,
    pub queue_acked: Arc<AtomicU64>,
    /// Kept alive so the queue worker's shutdown watch does not close.
    _shutdown: Arc<ShutdownSignal>,
}

impl FeatureServices {
    /// Build JWT/RBAC/storage/queue/mail services and spawn the queue worker.
    ///
    /// # Errors
    ///
    /// Returns setup errors for JWT keys, RBAC, or storage root creation.
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let jwt = Arc::new(JwtManager::new(
            JwtKey::new("smoke", b"render-modes-smoke-jwt-secret-32b!")?,
            JwtConfig::new(Duration::from_secs(3600)),
        )?);
        let authorizer = Arc::new(AuthorizationEngine::new(Rbac::build([
            Role::new("admin")?.allow("admin.open")?,
            Role::new("guest")?,
        ])?));
        let storage = LocalDisk::new("storage/feature-blobs")?;
        let backend = Arc::new(MemoryQueue::new());
        let queue = Arc::new(Queue::new(Arc::clone(&backend)));
        let queue_acked = Arc::new(AtomicU64::new(0));
        let acked = Arc::clone(&queue_acked);
        let shutdown = Arc::new(ShutdownSignal::new());
        let worker = Worker::new(
            backend,
            move |_job| {
                let acked = Arc::clone(&acked);
                async move {
                    acked.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            shutdown.token(),
        );
        tokio::spawn(async move {
            let _ = worker.run().await;
        });
        let (mailer, mail_sent) = Mailer::memory();
        Ok(Self {
            jwt,
            authorizer,
            storage,
            queue,
            mailer,
            mail_sent,
            queue_acked,
            _shutdown: shutdown,
        })
    }

    #[must_use]
    pub fn admin_permission() -> Permission {
        Permission::new("admin.open").expect("static permission")
    }
}

/// Install the greeter Feature for `/hello` and the `greet` console command.
///
/// # Errors
///
/// Returns a feature installation error when capabilities conflict.
pub fn plugins() -> Result<FeatureSet, FeatureError> {
    FeatureSet::new()
        .allow([Capability::Routes, Capability::Commands])
        .plugin(GreeterPlugin::new("smoke-hello"))
}
