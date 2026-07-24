//! Compile-time Feature / plugin installation for Phoenix-rs.
//!
//! Third-party crates implement [`Plugin`] and apps install them with
//! [`FeatureSet::plugin`]. There is no runtime dynamic loading.
//! See `docs/FEATURES.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use phoenix_console::CommandEntry;
#[cfg(feature = "database")]
use phoenix_database::Migration;
use phoenix_http::Middleware;
use phoenix_routing::{RouteGroup, Routes};
use thiserror::Error;

/// Declared ability a plugin may contribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Capability {
    /// HTTP routes via [`Plugin::routes`].
    Routes,
    /// Application console commands via [`Plugin::commands`].
    Commands,
    /// Database migrations via [`Plugin::migrations`].
    #[cfg(feature = "database")]
    Migrations,
}

impl Capability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Routes => "routes",
            Self::Commands => "commands",
            #[cfg(feature = "database")]
            Self::Migrations => "migrations",
        }
    }
}

/// An installable Feature (plugin) distributed as a Cargo crate.
pub trait Plugin: Send + Sync {
    /// Stable plugin id used in diagnostics and optional route-name prefixes.
    fn name(&self) -> &'static str;

    /// Plugin crate / package version string for diagnostics.
    fn version(&self) -> &'static str {
        "0.0.0"
    }

    /// Capabilities this plugin claims. Must cover every non-empty contribution.
    fn capabilities(&self) -> &'static [Capability];

    /// Routes contributed by this plugin. Prefer naming routes; path layout is
    /// the plugin author's choice (no forced URL prefix).
    fn routes(&self) -> Routes {
        Routes::new()
    }

    /// Console commands contributed by this plugin.
    fn commands(&self) -> Vec<CommandEntry> {
        Vec::new()
    }

    /// Migrations contributed by this plugin.
    #[cfg(feature = "database")]
    fn migrations(&self) -> Vec<Migration> {
        Vec::new()
    }
}

/// Collected contributions from one or more plugins.
pub struct FeatureSet {
    allow: Option<BTreeSet<Capability>>,
    namespace_route_names: bool,
    plugin_names: BTreeSet<&'static str>,
    command_names: BTreeSet<&'static str>,
    #[cfg(feature = "database")]
    migration_ids: BTreeSet<String>,
    routes: Routes,
    commands: Vec<CommandEntry>,
    #[cfg(feature = "database")]
    migrations: Vec<Migration>,
}

impl Default for FeatureSet {
    fn default() -> Self {
        Self::new()
    }
}

impl FeatureSet {
    /// Start an empty feature set. By default all known capabilities are allowed
    /// and route names are prefixed with `{plugin}.`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            allow: None,
            namespace_route_names: true,
            plugin_names: BTreeSet::new(),
            command_names: BTreeSet::new(),
            #[cfg(feature = "database")]
            migration_ids: BTreeSet::new(),
            routes: Routes::new(),
            commands: Vec::new(),
            #[cfg(feature = "database")]
            migrations: Vec::new(),
        }
    }

    /// Restrict which capabilities plugins may use. Unknown declarations fail.
    #[must_use]
    pub fn allow(mut self, capabilities: impl IntoIterator<Item = Capability>) -> Self {
        self.allow = Some(capabilities.into_iter().collect());
        self
    }

    /// When `true` (default), route names become `{plugin}.{name}`.
    #[must_use]
    pub fn namespace_route_names(mut self, enabled: bool) -> Self {
        self.namespace_route_names = enabled;
        self
    }

    /// Install one plugin. Fails closed on name / capability / contribution conflicts.
    ///
    /// # Errors
    ///
    /// Returns [`FeatureError`] when the plugin is invalid or conflicts with an
    /// already installed plugin.
    #[allow(clippy::needless_pass_by_value)] // owned install is the public ergonomic API
    pub fn plugin(mut self, plugin: impl Plugin) -> Result<Self, FeatureError> {
        let name = plugin.name();
        if name.trim().is_empty() {
            return Err(FeatureError::InvalidName(name));
        }
        if !self.plugin_names.insert(name) {
            return Err(FeatureError::DuplicatePlugin(name));
        }

        let declared: BTreeSet<_> = plugin.capabilities().iter().copied().collect();
        if let Some(allow) = &self.allow {
            for capability in &declared {
                if !allow.contains(capability) {
                    return Err(FeatureError::CapabilityDenied {
                        plugin: name,
                        capability: *capability,
                    });
                }
            }
        }

        let routes = plugin.routes();
        let commands = plugin.commands();
        #[cfg(feature = "database")]
        let migrations = plugin.migrations();

        ensure_capability(name, &declared, Capability::Routes, !routes.is_empty())?;
        ensure_capability(name, &declared, Capability::Commands, !commands.is_empty())?;
        #[cfg(feature = "database")]
        ensure_capability(
            name,
            &declared,
            Capability::Migrations,
            !migrations.is_empty(),
        )?;

        for command in &commands {
            if !self.command_names.insert(command.name()) {
                return Err(FeatureError::DuplicateCommand {
                    plugin: name,
                    command: command.name(),
                });
            }
        }
        #[cfg(feature = "database")]
        {
            for migration in &migrations {
                let id = migration.id().to_owned();
                if !self.migration_ids.insert(id.clone()) {
                    return Err(FeatureError::DuplicateMigration { plugin: name, id });
                }
            }
        }

        let routes = if self.namespace_route_names {
            routes.scoped(RouteGroup::new().name(format!("{name}.")))
        } else {
            routes
        };
        self.routes = self.routes.merge(routes);
        self.commands.extend(commands);
        #[cfg(feature = "database")]
        self.migrations.extend(migrations);
        let _ = plugin.version();
        Ok(self)
    }

    /// Attach shared middleware to every route contributed so far.
    #[must_use]
    pub fn with_middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        self.routes = self.routes.with_middleware(middleware);
        self
    }

    /// Take contributed routes for merging into the application router.
    #[must_use]
    pub fn into_routes(self) -> Routes {
        self.routes
    }

    /// Merge feature routes into an existing application routes table.
    #[must_use]
    pub fn merge_into(self, routes: Routes) -> Routes {
        routes.merge(self.routes)
    }

    /// Take console commands for chaining into [`phoenix_console::Console::commands`].
    #[must_use]
    pub fn into_commands(self) -> Vec<CommandEntry> {
        self.commands
    }

    /// Unpack routes, commands, and migrations when the app needs all three.
    #[must_use]
    pub fn into_parts(self) -> FeatureParts {
        FeatureParts {
            routes: self.routes,
            commands: self.commands,
            #[cfg(feature = "database")]
            migrations: self.migrations,
        }
    }

    /// Take migrations for the application [`phoenix_database::MigrationRunner`].
    #[must_use]
    #[cfg(feature = "database")]
    pub fn into_migrations(self) -> Vec<Migration> {
        self.migrations
    }

    /// Installed plugin names in sorted order.
    #[must_use]
    pub fn installed(&self) -> Vec<&'static str> {
        self.plugin_names.iter().copied().collect()
    }
}

/// Routes, commands, and migrations unpacked from a [`FeatureSet`].
pub struct FeatureParts {
    pub routes: Routes,
    pub commands: Vec<CommandEntry>,
    #[cfg(feature = "database")]
    pub migrations: Vec<Migration>,
}

fn ensure_capability(
    plugin: &'static str,
    declared: &BTreeSet<Capability>,
    capability: Capability,
    contributes: bool,
) -> Result<(), FeatureError> {
    if contributes && !declared.contains(&capability) {
        return Err(FeatureError::UndeclaredCapability { plugin, capability });
    }
    Ok(())
}

/// Feature installation error (fail closed).
#[derive(Debug, Error, Eq, PartialEq)]
pub enum FeatureError {
    #[error("plugin name must not be empty")]
    InvalidName(&'static str),
    #[error("duplicate plugin `{0}`")]
    DuplicatePlugin(&'static str),
    #[error(
        "plugin `{plugin}` is not allowed to use capability `{}`",
        capability.as_str()
    )]
    CapabilityDenied {
        plugin: &'static str,
        capability: Capability,
    },
    #[error(
        "plugin `{plugin}` contributes `{}` but did not declare that capability",
        capability.as_str()
    )]
    UndeclaredCapability {
        plugin: &'static str,
        capability: Capability,
    },
    #[error("plugin `{plugin}` registers duplicate command `{command}`")]
    DuplicateCommand {
        plugin: &'static str,
        command: &'static str,
    },
    #[error("plugin `{plugin}` registers duplicate migration `{id}`")]
    #[cfg(feature = "database")]
    DuplicateMigration { plugin: &'static str, id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::{IntoResponse, Json, Request};
    use serde_json::json;

    struct HelloPlugin;

    impl Plugin for HelloPlugin {
        fn name(&self) -> &'static str {
            "hello"
        }

        fn capabilities(&self) -> &'static [Capability] {
            &[Capability::Routes, Capability::Commands]
        }

        fn routes(&self) -> Routes {
            Routes::new()
                .get("/hello", |_request: Request| async {
                    Json(json!({ "ok": true })).into_response()
                })
                .name("index")
        }

        fn commands(&self) -> Vec<CommandEntry> {
            vec![CommandEntry::new("hello", |_ctx| {
                Box::pin(async { Ok(()) })
            })]
        }
    }

    struct UndeclaredRoutes;

    impl Plugin for UndeclaredRoutes {
        fn name(&self) -> &'static str {
            "bad"
        }

        fn capabilities(&self) -> &'static [Capability] {
            &[]
        }

        fn routes(&self) -> Routes {
            Routes::new()
                .get("/x", |_request: Request| async { "x".into_response() })
                .name("x")
        }
    }

    #[test]
    fn installs_routes_and_commands_with_namespace() {
        let parts = FeatureSet::new().plugin(HelloPlugin).unwrap().into_parts();
        assert_eq!(parts.commands.len(), 1);
        assert_eq!(parts.commands[0].name(), "hello");
        let router = parts.routes.build().unwrap();
        assert_eq!(router.url("hello.index", &[]).unwrap(), "/hello");
    }

    #[test]
    fn rejects_undeclared_and_denied_capabilities() {
        assert!(matches!(
            FeatureSet::new().plugin(UndeclaredRoutes),
            Err(FeatureError::UndeclaredCapability {
                capability: Capability::Routes,
                ..
            })
        ));
        assert!(matches!(
            FeatureSet::new()
                .allow([Capability::Commands])
                .plugin(HelloPlugin),
            Err(FeatureError::CapabilityDenied {
                capability: Capability::Routes,
                ..
            })
        ));
    }

    #[test]
    fn rejects_duplicate_plugins() {
        let result = FeatureSet::new()
            .plugin(HelloPlugin)
            .and_then(|set| set.plugin(HelloPlugin));
        assert_eq!(result.err(), Some(FeatureError::DuplicatePlugin("hello")));
    }
}
