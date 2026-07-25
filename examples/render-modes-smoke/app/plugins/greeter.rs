//! Local copy of the greeter Feature (same shape as `examples/plugin-greeter`)
//! so this independent workspace can demonstrate `FeatureSet` without depending
//! on the root workspace package metadata.

use phoenix::plugin::{Capability, Plugin};
use phoenix::prelude::*;
use serde_json::json;

pub struct GreeterPlugin {
    greeting: String,
}

impl GreeterPlugin {
    #[must_use]
    pub fn new(greeting: impl Into<String>) -> Self {
        Self {
            greeting: greeting.into(),
        }
    }
}

impl Plugin for GreeterPlugin {
    fn name(&self) -> &'static str {
        "greeter"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Routes, Capability::Commands]
    }

    fn routes(&self) -> Routes {
        let greeting = self.greeting.clone();
        Routes::new()
            .get("/hello", move |_request: Request| {
                let greeting = greeting.clone();
                async move { Json(json!({ "message": greeting })).into_response() }
            })
            .name("hello")
    }

    fn commands(&self) -> Vec<CommandEntry> {
        let greeting = self.greeting.clone();
        vec![CommandEntry::new("greet", move |_ctx| {
            let greeting = greeting.clone();
            Box::pin(async move {
                println!("{greeting}");
                Ok(())
            })
        })]
    }
}
