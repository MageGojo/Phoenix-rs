//! Example Feature crate: install with `FeatureSet::plugin(GreeterPlugin::new("…"))`.
//!
//! See `docs/FEATURES.md`.

use phoenix::plugin::{Capability, Plugin};
use phoenix::prelude::*;
use serde_json::json;

/// Demo plugin that exposes `GET /hello` and a `greet` console command.
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

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix::plugin::FeatureSet;

    #[test]
    fn greeter_installs_under_namespace() {
        let parts = FeatureSet::new()
            .plugin(GreeterPlugin::new("hi"))
            .unwrap()
            .into_parts();
        assert_eq!(parts.commands[0].name(), "greet");
        let router = parts.routes.build().unwrap();
        assert_eq!(router.url("greeter.hello", &[]).unwrap(), "/hello");
    }
}
