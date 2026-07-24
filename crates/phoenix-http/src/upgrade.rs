//! Hyper connection upgrade handle shared by protocol facades.

use std::sync::{Arc, Mutex};

use hyper::upgrade::OnUpgrade;

/// Takeable Hyper upgrade handle installed by the HTTP/1 connection layer.
#[derive(Clone, Debug)]
pub struct ConnectionUpgrade {
    on_upgrade: Arc<Mutex<Option<OnUpgrade>>>,
}

impl ConnectionUpgrade {
    /// Wrap a Hyper [`OnUpgrade`] so handlers can take it through `&Request`.
    #[must_use]
    pub fn new(on_upgrade: OnUpgrade) -> Self {
        Self {
            on_upgrade: Arc::new(Mutex::new(Some(on_upgrade))),
        }
    }

    /// Take the pending upgrade exactly once.
    #[must_use]
    pub fn take(&self) -> Option<OnUpgrade> {
        self.on_upgrade.lock().ok()?.take()
    }
}
