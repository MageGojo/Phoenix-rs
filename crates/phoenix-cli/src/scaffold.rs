use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

const MODULES_START: &str = "// <phoenix:modules>";
const MODULES_END: &str = "// </phoenix:modules>";
const MODELS_START: &str = "// <phoenix:model-registry>";
const MODELS_END: &str = "// </phoenix:model-registry>";
const MIGRATIONS_START: &str = "// <phoenix:migration-registry>";
const MIGRATIONS_END: &str = "// </phoenix:migration-registry>";
const COMMANDS_START: &str = "// <phoenix:commands>";
const COMMANDS_END: &str = "// </phoenix:commands>";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencySource {
    Registry,
    Local(PathBuf),
}

impl DependencySource {
    #[must_use]
    pub fn discover() -> Self {
        if let Some(path) = env::var_os("PHOENIX_FRAMEWORK_PATH") {
            let path = PathBuf::from(path);
            if is_framework_root(&path) {
                return Self::Local(path);
            }
        }
        let Ok(executable) = env::current_exe() else {
            return Self::Registry;
        };
        for ancestor in executable.ancestors() {
            if is_framework_root(ancestor) {
                return Self::Local(ancestor.to_path_buf());
            }
        }
        let build_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        if is_framework_root(&build_root) {
            return Self::Local(build_root);
        }
        Self::Registry
    }
}

fn is_framework_root(path: &Path) -> bool {
    path.join("crates/phoenix/Cargo.toml").is_file()
        && path.join("packages/phoenix-react/package.json").is_file()
        && path.join("packages/phoenix-vite/package.json").is_file()
}

#[derive(Clone, Debug)]
pub struct NewProjectOptions {
    pub target: PathBuf,
    pub dependencies: DependencySource,
    pub initialize_git: bool,
    pub install_dependencies: bool,
}

impl NewProjectOptions {
    #[must_use]
    pub fn new(target: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            dependencies: DependencySource::discover(),
            initialize_git: true,
            install_dependencies: true,
        }
    }

    #[must_use]
    pub fn dependencies(mut self, dependencies: DependencySource) -> Self {
        self.dependencies = dependencies;
        self
    }

    #[must_use]
    pub const fn initialize_git(mut self, initialize: bool) -> Self {
        self.initialize_git = initialize;
        self
    }

    #[must_use]
    pub const fn install_dependencies(mut self, install: bool) -> Self {
        self.install_dependencies = install;
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GenerateOptions {
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerOptions {
    pub force: bool,
    pub resource: bool,
    pub route: bool,
}

#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModelOptions {
    pub all: bool,
    pub api_resource: bool,
    pub controller: bool,
    pub force: bool,
    pub migration: bool,
    pub page: bool,
    pub request: bool,
    pub resource_controller: bool,
}

#[derive(Debug, Error)]
pub enum ScaffoldError {
    #[error("invalid Phoenix name `{0}`; use letters, numbers, dashes, underscores, / or ::")]
    InvalidName(String),
    #[error("project target {0} already exists and is not empty")]
    ProjectNotEmpty(PathBuf),
    #[error("{0} is not a Phoenix project root")]
    NotProject(PathBuf),
    #[error("refusing to overwrite existing file {0}; pass --force to replace it")]
    AlreadyExists(PathBuf),
    #[error("Phoenix managed markers are missing or malformed in {0}")]
    InvalidManagedFile(PathBuf),
    #[error("local Phoenix framework root is invalid: {0}")]
    InvalidFrameworkRoot(PathBuf),
    #[error("failed to read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{program} exited unsuccessfully while preparing the project")]
    CommandFailed { program: &'static str },
    #[error("the current time is before the Unix epoch")]
    InvalidClock,
}

/// Create a complete Phoenix application that can immediately run `px dev`.
///
/// # Errors
///
/// Returns an error for invalid names, non-empty targets, invalid local framework
/// paths, file-system failures, or dependency/bootstrap command failures.
pub fn create_project(options: &NewProjectOptions) -> Result<(), ScaffoldError> {
    let target = absolute_path(&options.target)?;
    ensure_empty_target(&target)?;
    let directory_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ScaffoldError::InvalidName(target.display().to_string()))?;
    let package = package_name(directory_name)?;
    if let DependencySource::Local(root) = &options.dependencies
        && !is_framework_root(root)
    {
        return Err(ScaffoldError::InvalidFrameworkRoot(root.clone()));
    }

    let mut editor = ProjectEditor::new(&target, false);
    for (path, content) in project_files(&package, &options.dependencies)? {
        editor.create(path, content)?;
    }
    editor.commit()?;

    if options.initialize_git {
        run_optional("git", &["init", "--quiet"], &target)?;
    }
    if options.install_dependencies {
        run_required("npm", &["install"], &target)?;
        run_required("npm", &["run", "types", "--silent"], &target)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ProjectGenerator {
    root: PathBuf,
}

impl ProjectGenerator {
    /// Locate the Phoenix project containing `start`.
    ///
    /// # Errors
    ///
    /// Returns an error when no parent contains the expected Phoenix layout.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, ScaffoldError> {
        let start = absolute_path(start.as_ref())?;
        for candidate in start.ancestors() {
            if is_project_root(candidate) {
                return Ok(Self {
                    root: candidate.to_path_buf(),
                });
            }
        }
        Err(ScaffoldError::NotProject(start))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Refresh generated TypeScript contracts after a Rust generator runs.
    ///
    /// Returns `Ok(false)` before JavaScript dependencies are installed; Vite
    /// will still generate the files when development starts.
    ///
    /// # Errors
    ///
    /// Returns an error when the installed Phoenix Vite generator fails.
    pub fn refresh_types(&self) -> Result<bool, ScaffoldError> {
        if !self.root.join("node_modules/@phoenix/vite").is_dir() {
            return Ok(false);
        }
        run_required("npm", &["run", "types", "--silent"], &self.root)?;
        Ok(true)
    }

    /// Generate a controller and optionally its conventional resource route.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn controller(
        &self,
        name: &str,
        options: ControllerOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let name = QualifiedName::parse_with_suffix(name, "Controller")?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_rust_item(
            &mut editor,
            "app/controllers",
            &name,
            &controller_template(&name.class, options.resource),
        )?;
        if options.route || options.resource {
            add_controller_route(&mut editor, &name, options.resource, None)?;
        }
        editor.commit()
    }

    /// Generate a Toasty model and any requested companion artifacts.
    ///
    /// `--all` creates the currently supported cohesive business slice: model,
    /// migration, request, API resource, resource controller/route, and index page.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn model(
        &self,
        name: &str,
        mut options: ModelOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        if options.all {
            options.migration = true;
            options.request = true;
            options.api_resource = true;
            options.controller = true;
            options.resource_controller = true;
            options.page = true;
        }
        let model = QualifiedName::parse(name)?;
        let request = model.with_leaf(format!("Store{}Request", model.class));
        let resource = model.with_leaf(format!("{}Resource", model.class));
        let controller = model.with_leaf(format!("{}Controller", model.class));
        let page = model.index_page_name();
        let props = page_props_name(&page);
        let cohesive = options.request
            && options.api_resource
            && options.controller
            && options.resource_controller
            && options.page;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_model(&mut editor, &model)?;
        if options.migration {
            add_model_migration(&mut editor, &model)?;
        }
        if options.request {
            add_rust_item(
                &mut editor,
                "app/requests",
                &request,
                &request_template(&request.class),
            )?;
        }
        if options.api_resource {
            add_rust_item(
                &mut editor,
                "app/resources",
                &resource,
                &resource_template(&resource.class),
            )?;
        }
        if options.controller || options.resource_controller {
            let content = if cohesive {
                model_controller_template(&controller, &request, &resource, &props, &page.route)
            } else {
                controller_template(&controller.class, options.resource_controller)
            };
            add_rust_item(&mut editor, "app/controllers", &controller, &content)?;
            let action = cohesive.then_some((&request, &resource));
            add_controller_route(
                &mut editor,
                &controller,
                options.resource_controller,
                action,
            )?;
        }
        if options.page {
            add_page(&mut editor, &page)?;
        }
        editor.commit()
    }

    /// Generate one migration and register it in `database/migrations/mod.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, time, or managed files.
    pub fn migration(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let migration_name = snake_identifier(name)?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_migration(
            &mut editor,
            &migration_name,
            inferred_table(&migration_name),
        )?;
        editor.commit()
    }

    /// Generate a validated request DTO and update its Rust module.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn request(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        self.rust_contract(name, "Request", "app/requests", request_template, options)
    }

    /// Generate a browser-safe API resource and update its Rust module.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn resource(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        self.rust_contract(
            name,
            "Resource",
            "app/resources",
            resource_template,
            options,
        )
    }

    /// Generate a pass-through middleware ready for application logic.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn middleware(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let name = QualifiedName::parse_with_suffix(name, "Middleware")?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_rust_item(
            &mut editor,
            "app/middleware",
            &name,
            &middleware_template(&name.class),
        )?;
        editor.commit()
    }

    /// Generate a React page plus its Rust Page Props contract.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn page(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let page = PageName::parse(name)?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_page(&mut editor, &page)?;
        editor.commit()
    }

    /// Generate a React Island component.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or file-system failures.
    pub fn island(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let name = QualifiedName::parse(name)?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        let mut path = PathBuf::from("views/islands");
        path.extend(name.modules.iter().map(|module| kebab_case(module)));
        path.push(format!("{}.tsx", kebab_case(&name.class)));
        editor.create(path, island_template(&name.class))?;
        editor.commit()
    }

    /// Generate a console command and register it in `app/commands/mod.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, conflicts, or malformed managed files.
    pub fn command(
        &self,
        name: &str,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let name = QualifiedName::parse(name)?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_command(&mut editor, &name)?;
        editor.commit()
    }

    fn rust_contract(
        &self,
        name: &str,
        suffix: &str,
        directory: &str,
        template: fn(&str) -> String,
        options: GenerateOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let name = QualifiedName::parse_with_suffix(name, suffix)?;
        let mut editor = ProjectEditor::new(&self.root, options.force);
        add_rust_item(&mut editor, directory, &name, &template(&name.class))?;
        editor.commit()
    }
}

fn is_project_root(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("package.json").is_file()
        && path.join("app").is_dir()
        && path.join("routes").is_dir()
        && path.join("views").is_dir()
}

fn project_files(
    package: &str,
    dependencies: &DependencySource,
) -> Result<Vec<(PathBuf, String)>, ScaffoldError> {
    let (rust_dependency, react, react_ssr, vite) = match dependencies {
        DependencySource::Registry => (
            // crates.io package is `phoenixrs` (phoenix / phoenix-rs taken);
            // lib crate remains `phoenix` so apps keep `use phoenix::…`.
            "phoenix = { package = \"phoenixrs\", version = \"0.1.0\" }".to_owned(),
            "0.1.0".to_owned(),
            "0.1.0".to_owned(),
            "0.1.0".to_owned(),
        ),
        DependencySource::Local(root) => {
            let root = absolute_path(root)?;
            (
                format!(
                    "phoenix = {{ package = \"phoenixrs\", path = {} }}",
                    json_string(&root.join("crates/phoenix").to_string_lossy())
                ),
                format!("file:{}", root.join("packages/phoenix-react").display()),
                format!("file:{}", root.join("packages/phoenix-react-ssr").display()),
                format!("file:{}", root.join("packages/phoenix-vite").display()),
            )
        }
    };
    let crate_name = package.replace('-', "_");
    let package_json = format!(
        r#"{{
  "name": {package},
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "vite --host 127.0.0.1",
    "build": "npm run build:client && npm run build:ssr",
    "build:client": "vite build",
    "build:ssr": "vite build --config vite.ssr.config.ts",
    "types": "node -e \"import('@phoenix/vite').then(({{ generateRouteTypes }}) => generateRouteTypes('routes', 'views/generated/routes.ts', '.', 'views/generated/contracts.ts'))\"",
    "typecheck": "npm run types && tsc --noEmit"
  }},
  "dependencies": {{
    "@phoenix/react": {react},
    "@phoenix/react-ssr": {react_ssr},
    "@phoenix/vite": {vite},
    "react": "^19.1.0",
    "react-dom": "^19.1.0"
  }},
  "devDependencies": {{
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "typescript": "^5.8.0",
    "vite": "^7.3.6"
  }}
}}
"#,
        package = json_string(package),
        react = json_string(&react),
        react_ssr = json_string(&react_ssr),
        vite = json_string(&vite),
    );

    Ok(vec![
        (
            "Cargo.toml".into(),
            format!(
                "[package]\nname = {package}\nversion = \"0.1.0\"\nedition = \"2024\"\nrust-version = \"1.95\"\npublish = false\ndefault-run = {package}\n\n[dependencies]\n{rust_dependency}\nserde = {{ version = \"1\", features = [\"derive\"] }}\nserde_json = \"1\"\ntoasty = {{ version = \"0.8\", features = [\"migration\", \"mysql\", \"postgresql\", \"serde\", \"sqlite\"] }}\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\", \"signal\"] }}\n\n[workspace]\n",
                package = json_string(package),
            ),
        ),
        ("package.json".into(), package_json),
        (".gitignore".into(), "/target\n/node_modules\n/public/assets\n/public/ssr\n/views/generated/*.ts\n/dist\n.env\n.DS_Store\n".to_owned()),
        (".env.example".into(), env_example_template()),
        ("README.md".into(), project_readme(package)),
        ("src/main.rs".into(), main_template(&crate_name)),
        (
            "src/bin/phoenix-manage.rs".into(),
            management_template(&crate_name),
        ),
        ("src/lib.rs".into(), lib_template()),
        ("config/mod.rs".into(), config_template()),
        ("config/app.toml".into(), app_toml_template(package)),
        ("config/database.toml".into(), database_toml_template()),
        (
            "config/schemas/phoenix-config-app.schema.json".into(),
            include_str!("../schemas/phoenix-config-app.schema.json").to_owned(),
        ),
        (
            "config/schemas/phoenix-config-database.schema.json".into(),
            include_str!("../schemas/phoenix-config-database.schema.json").to_owned(),
        ),
        ("taplo.toml".into(), app_taplo_template()),
        ("deploy/restart.sh.example".into(), deploy_restart_example()),
        ("app/controllers/mod.rs".into(), managed_modules(&["pub mod home_controller;", "pub use home_controller::HomeController;"])),
        ("app/controllers/home_controller.rs".into(), home_controller_template()),
        ("app/props/mod.rs".into(), managed_modules(&["pub mod home_props;", "pub use home_props::HomeProps;"])),
        ("app/props/home_props.rs".into(), home_props_template()),
        ("app/models/mod.rs".into(), empty_model_registry()),
        ("app/requests/mod.rs".into(), managed_modules(&[])),
        ("app/resources/mod.rs".into(), managed_modules(&[])),
        ("app/middleware/mod.rs".into(), managed_modules(&[])),
        ("app/commands/mod.rs".into(), commands_mod_template()),
        (
            "database/migrations/mod.rs".into(),
            empty_migration_registry(),
        ),
        ("database/seeders/mod.rs".into(), seeder_template()),
        ("routes/web.rs".into(), home_route_template()),
        ("views/pages/home.tsx".into(), home_page_template()),
        ("views/styles.css".into(), styles_template()),
        ("views/generated/contracts.ts".into(), generated_contracts_template()),
        ("views/generated/routes.ts".into(), generated_routes_template()),
        ("vite.config.ts".into(), vite_template(false)),
        ("vite.ssr.config.ts".into(), vite_template(true)),
        ("tsconfig.json".into(), tsconfig_template()),
        ("public/.gitkeep".into(), String::new()),
        ("storage/cache/.gitkeep".into(), String::new()),
        ("storage/logs/.gitkeep".into(), String::new()),
        ("views/components/.gitkeep".into(), String::new()),
        ("views/islands/.gitkeep".into(), String::new()),
        ("views/layouts/.gitkeep".into(), String::new()),
    ])
}

fn env_example_template() -> String {
    r#"# Copy to `.env` for local secrets and overrides.
# Structured defaults live in config/app.toml and config/database.toml.
# Precedence: config/*.toml < .env < process environment.

APP_ENV=development
APP_ADDR=127.0.0.1:3000
APP_URL=http://127.0.0.1:3000

# Database: prefer editing config/database.toml default = "sqlite" | "pgsql" | "mysql".
# Optional overrides:
# DB_CONNECTION=pgsql
# DB_CONNECTION=mysql
# DB_PASSWORD=secret
# DATABASE_URL=postgresql://phoenix:secret@127.0.0.1:5432/phoenix
# DATABASE_URL=mysql://phoenix:secret@127.0.0.1:3306/phoenix

TRUSTED_PROXIES=none
ALLOWED_HOSTS=127.0.0.1,localhost,[::1]
RATE_LIMIT_REQUESTS=60
RATE_LIMIT_WINDOW_SECONDS=60
VITE_DEV_URL=http://127.0.0.1:5173
PHOENIX_LOG=info,hyper=warn
"#
    .to_owned()
}

fn app_toml_template(package: &str) -> String {
    format!(
        r#"# Application settings (Laravel-style config/app).
# Secrets and machine-specific overrides belong in `.env`.
# Editor autocomplete: Even Better TOML / Taplo + #:schema below.

#:schema ./schemas/phoenix-config-app.schema.json

name = {package}
env = "development"
addr = "127.0.0.1:3000"
url = "http://127.0.0.1:3000"
"#,
        package = json_string(package),
    )
}

fn database_toml_template() -> String {
    r#"# Database connections (Laravel-style config/database).
#
# Switch engines by changing `default`:
#   default = "sqlite"   # local file, zero setup
#   default = "pgsql"    # PostgreSQL
#   default = "mysql"    # MySQL / MariaDB
#
# Or set DB_CONNECTION=pgsql|mysql in `.env` without editing this file.
# Put DB_PASSWORD in `.env` — do not commit production passwords here.
# Editor autocomplete: Even Better TOML / Taplo + #:schema below.

#:schema ./schemas/phoenix-config-database.schema.json

default = "sqlite"

[connections.sqlite]
driver = "sqlite"
# Path is relative to the application root (creates parent dirs as needed by the OS/driver).
database = "storage/app.sqlite"

[connections.pgsql]
driver = "pgsql"
host = "127.0.0.1"
port = 5432
database = "phoenix"
username = "phoenix"
password = ""

[connections.mysql]
driver = "mysql"
host = "127.0.0.1"
port = 3306
database = "phoenix"
username = "phoenix"
password = ""
"#
    .to_owned()
}

fn app_taplo_template() -> String {
    r#"# Taplo / Even Better TOML schema associations for config/*.toml autocomplete.

[[rule]]
include = ["config/app.toml"]
[rule.schema]
path = "./config/schemas/phoenix-config-app.schema.json"

[[rule]]
include = ["config/database.toml"]
[rule.schema]
path = "./config/schemas/phoenix-config-database.schema.json"
"#
    .to_owned()
}

fn deploy_restart_example() -> String {
    r"#!/bin/sh
# Copy to deploy/restart.sh and make executable.
# Used by `px release:install` / `px release:rollback` when --restart-cmd is omitted.
set -eu
systemctl restart my-app
"
    .to_owned()
}

fn config_template() -> String {
    r#"pub use phoenix::config::{AppConfig, AppConfigBuilder, ConfigError, Environment, SecretValue};

/// Load this application's configuration.
///
/// Reads `config/app.toml` + `config/database.toml`, then `.env`, then process
/// environment. To require JWT/encryption secrets in production:
/// `AppConfig::builder().required_secret("JWT_SECRET", 32).load()`.
///
/// # Errors
///
/// Returns a source, validation, or production-requirement error.
pub fn load() -> Result<AppConfig, ConfigError> {
    AppConfig::load()
}
"#
    .to_owned()
}

#[allow(clippy::too_many_lines)]
fn management_template(crate_name: &str) -> String {
    r#"use std::{env, error::Error, io};

use phoenix::database::MigrationRunner;

type CommandResult<T = ()> = Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> CommandResult {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let command = arguments
        .first()
        .map(String::as_str)
        .ok_or_else(|| input_error("expected migrate, status, rollback, fresh, or seed"))?;
    let options = &arguments[1..];
    if !matches!(command, "migrate" | "status" | "rollback" | "fresh" | "seed") {
        return Err(input_error(format!("unknown management command `{command}`")).into());
    }

    let config = __PHOENIX_APP_CRATE__::config::load()?;
    let mut database = __PHOENIX_APP_CRATE__::database(&config).await?;
    if command == "seed" {
        require_no_options(options)?;
        __PHOENIX_APP_CRATE__::seeders::run(&mut database).await?;
        println!("Seeders completed.");
        return Ok(());
    }

    let mut runner = MigrationRunner::new(
        &mut database,
        __PHOENIX_APP_CRATE__::migrations::all(),
    )?;
    match command {
        "migrate" => {
            require_no_options(options)?;
            let applied = runner.up().await?;
            println!("Applied {applied} migration(s).");
        }
        "status" => {
            require_no_options(options)?;
            let plan = runner.plan().await?;
            if plan.applied.is_empty() && plan.pending.is_empty() {
                println!("No migrations registered or applied.");
            }
            for migration in plan.applied {
                println!(
                    "APPLIED  {}  batch={}  {}  {}",
                    migration.id, migration.batch, migration.applied_at, migration.name
                );
            }
            for id in plan.pending {
                println!("PENDING  {id}");
            }
        }
        "rollback" => {
            let steps = parse_rollback_steps(options)?;
            let rolled_back = runner.down(steps).await?;
            println!("Rolled back {rolled_back} migration(s).");
        }
        "fresh" => {
            let run_seeders = parse_fresh_options(options)?;
            let applied = runner.plan().await?.applied.len();
            let rolled_back = runner.down(applied).await?;
            let migrated = runner.up().await?;
            println!(
                "Rebuilt the database: rolled back {rolled_back}, applied {migrated} migration(s)."
            );
            drop(runner);
            if run_seeders {
                __PHOENIX_APP_CRATE__::seeders::run(&mut database).await?;
                println!("Seeders completed.");
            }
        }
        "seed" => unreachable!("seed is handled before creating the migration runner"),
        _ => unreachable!("management commands are validated before connecting"),
    }
    Ok(())
}

fn require_no_options(options: &[String]) -> CommandResult {
    if options.is_empty() {
        Ok(())
    } else {
        Err(input_error(format!("unexpected arguments: {}", options.join(" "))).into())
    }
}

fn parse_rollback_steps(options: &[String]) -> CommandResult<usize> {
    let [steps] = options else {
        return Err(input_error("rollback expects one positive step count").into());
    };
    steps
        .parse::<usize>()
        .ok()
        .filter(|steps| *steps > 0)
        .ok_or_else(|| input_error("rollback step count must be a positive integer").into())
}

fn parse_fresh_options(options: &[String]) -> CommandResult<bool> {
    match options {
        [] => Ok(false),
        [option] if option == "--seed" => Ok(true),
        _ => Err(input_error("fresh only accepts --seed").into()),
    }
}

fn input_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
"#
    .replace("__PHOENIX_APP_CRATE__", crate_name)
}

fn seeder_template() -> String {
    r"use std::error::Error;

use phoenix::database::Database;

/// Insert repeatable development or test data.
///
/// # Errors
///
/// Returns the first application or database error raised by a seeder.
pub async fn run(_database: &mut Database) -> Result<(), Box<dyn Error>> {
    Ok(())
}
"
    .to_owned()
}

fn empty_model_registry() -> String {
    format!(
        "{MODULES_START}\n{MODULES_END}\n\n{MODELS_START}\n{}\n{MODELS_END}\n",
        render_model_registry(&BTreeSet::new()).join("\n")
    )
}

fn empty_migration_registry() -> String {
    format!(
        "{MIGRATIONS_START}\n{}\n{MIGRATIONS_END}\n",
        render_migration_registry(&BTreeSet::new()).join("\n")
    )
}

fn project_readme(package: &str) -> String {
    format!(
        "# {package}\n\nPhoenix Rust + React application.\n\n## Start\n\n```bash\ncp .env.example .env\nnpm install\npx migrate\npx dev\n```\n\nOpen <http://127.0.0.1:3000>.\n\n## Configuration\n\nLaravel-style TOML lives in `config/`:\n\n- `config/app.toml` — app name, env, listen address, public URL\n- `config/database.toml` — **choose the database** with `default = \"sqlite\"`, `\"pgsql\"`, or `\"mysql\"`\n\nPut secrets in `.env` (for example `DB_PASSWORD`). Precedence: `config/*.toml` < `.env` < process environment.\n\nEditor autocomplete for `config/*.toml` uses JSON Schema (`config/schemas/`) via Taplo / Even Better TOML (`taplo.toml`).\n\nThird-party Features (plugins): implement `Plugin`, then `FeatureSet::new().plugin(...)` and `.merge(features.into_routes())`. See Phoenix-rs `docs/FEATURES.md`.\n\n## Release\n\n```bash\npx release --version 0.1.0 --tarball\n# upload dist/releases/.../*.tar.gz, then on the server:\n# export PHOENIX_DEPLOY_ROOT=/var/www/my-app\n# px release:install --tarball /path/to/app-0.1.0.tar.gz --version 0.1.0\n# px release:rollback --steps 1\n```\n\nSee Phoenix-rs `docs/RELEASE_PIPELINE.md`.\n\n## Console\n\n```bash\ncargo run -- serve\ncargo run -- update\ncargo run -- help\n```\n\n## Database\n\n```bash\npx status\npx migrate\npx rollback --step 1\npx fresh --seed\npx seed\n```\n\nMigrations are registered in `database/migrations/mod.rs`. Add repeatable development data in `database/seeders/mod.rs`.\n\nProduction startup requires explicit `APP_URL`, database settings, `TRUSTED_PROXIES`, and `ALLOWED_HOSTS` values. Use `TRUSTED_PROXIES=none` when the service has no trusted reverse proxy. Declare purpose-specific JWT or encryption keys with `AppConfigBuilder::required_secret` only when the corresponding service consumes them.\n\n## Generate business code\n\n```bash\npx make:model Post --all\npx make:controller AdminController\npx make:request StorePostRequest\npx make:resource PostResource\npx make:middleware RequireLoginMiddleware\npx make:page posts/index\npx make:island LikeButton\npx make:command Update\n```\n"
    )
}

fn main_template(crate_name: &str) -> String {
    format!(
        r#"use phoenix::prelude::{{CommandResult, Console, LogFormat, Logging}};

use {crate_name}::commands;

#[tokio::main]
async fn main() -> CommandResult {{
    Console::new(env!("CARGO_PKG_NAME"))
        .about("Phoenix application")
        .serve(|_ctx| async move {{
            let config = {crate_name}::config::load()?;
            let address = config.address().to_owned();
            let public_url = config.public_url().to_owned();
            let production = config.environment().is_production();
            let _logging = Logging::new()
                .format(if production {{ LogFormat::Json }} else {{ LogFormat::Compact }})
                .ansi(!production)
                .init()?;
            let server = {crate_name}::application(config)?.bind(&address).await?;
            println!(
                "Phoenix application ready at {{public_url}} (listening on {{}})",
                server.local_addr()
            );
            server
                .run_with_shutdown(async {{
                    let _ = tokio::signal::ctrl_c().await;
                }})
                .await?;
            Ok(())
        }})
        .commands(commands::registry())
        .run()
        .await
}}
"#
    )
}

fn lib_template() -> String {
    r#"#[path = "../config/mod.rs"]
pub mod config;
#[path = "../app/commands/mod.rs"]
pub mod commands;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[path = "../database/migrations/mod.rs"]
pub mod migrations;
#[path = "../database/seeders/mod.rs"]
pub mod seeders;

use phoenix::prelude::{
    AccessLog, Application, Csrf, Database, DatabaseError, HostAllowlist, NonceSecurityPolicy,
    RateLimit, RateLimitConfig, RequestId, RouteBuildError, Routes, SessionConfig,
    SessionMiddleware, SessionStore, StateMiddleware, TrustedProxies,
};

use config::AppConfig;

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes(config: &AppConfig) -> Routes {
    let session_config = SessionConfig {
        secure: config.public_url().starts_with("https://"),
        ..SessionConfig::default()
    };
    let session_store = SessionStore::memory(session_config.max_age);

    phoenix::mount_routes!()
        .with_middleware(TrustedProxies::new(config.trusted_proxies().iter().copied()))
        .with_middleware(RequestId)
        .with_middleware(AccessLog)
        .with_middleware(HostAllowlist::new(config.allowed_hosts().iter().cloned()))
        .with_middleware(RateLimit::new(RateLimitConfig {
            requests: config.rate_limit_requests(),
            window: config.rate_limit_window(),
        }))
        .with_middleware(content_security_policy(config))
        .with_middleware(SessionMiddleware::new(session_store, session_config))
        .with_middleware(Csrf)
        .with_middleware(StateMiddleware::new(config.clone()))
}

fn content_security_policy(config: &AppConfig) -> NonceSecurityPolicy {
    if !config.environment().is_production() {
        return NonceSecurityPolicy::development(
            config
                .vite_dev_url()
                .expect("development configuration always has a Vite origin"),
        )
        .expect("AppConfig validates VITE_DEV_URL as one trusted HTTP(S) origin");
    }
    NonceSecurityPolicy::default()
}

/// Build the Phoenix application.
///
/// # Errors
///
/// Returns a route error when route names or patterns conflict.
pub fn application(config: AppConfig) -> Result<Application, RouteBuildError> {
    Application::new(routes(&config))
}

/// Connect the configured database with every registered Toasty model.
///
/// # Errors
///
/// Returns a database error when the URL or connection is invalid.
pub async fn database(config: &AppConfig) -> Result<Database, DatabaseError> {
    Database::builder(models::all())
        .connect(config.database_url())
        .await
}
"#
    .to_owned()
}

fn commands_mod_template() -> String {
    format!(
        "use phoenix::prelude::commands;\n\n{MODULES_START}\n{MODULES_END}\n\ncommands! {{\n{COMMANDS_START}\n{COMMANDS_END}\n}}\n"
    )
}

fn command_template(function_name: &str) -> String {
    format!(
        r#"use phoenix::prelude::{{CommandContext, CommandResult}};

/// Application console command.
#[allow(clippy::unused_async)]
pub async fn {function_name}(_ctx: CommandContext<'_>) -> CommandResult {{
    println!("{function_name} ran.");
    Ok(())
}}
"#
    )
}

fn home_controller_template() -> String {
    r#"use phoenix::prelude::{Page, PageResponseError, Request, Response};

use crate::props::HomeProps;

pub struct HomeController;

impl HomeController {
    pub async fn index(request: Request) -> Result<Response, PageResponseError> {
        Page::new(
            "home",
            HomeProps {
                title: "Phoenix is ready".to_owned(),
                description: "Rust owns the application contract; React renders the page.".to_owned(),
            },
        )
        .spa()
        .respond_to(&request, None)
    }
}
"#.to_owned()
}

fn home_props_template() -> String {
    r#"use serde::Serialize;

#[phoenix::contract(page, page = "home")]
#[derive(Serialize)]
pub struct HomeProps {
    pub title: String,
    pub description: String,
}
"#
    .to_owned()
}

fn home_route_template() -> String {
    r#"use phoenix::prelude::Routes;

use crate::controllers::HomeController;

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .get("/", HomeController::index)
        .name("home")
}
"#
    .to_owned()
}

fn home_page_template() -> String {
    r#"import type { HomeProps } from "../generated/contracts.js";

export default function Home({ title, description }: HomeProps) {
  return (
    <main className="welcome">
      <p className="eyebrow">PHOENIX / RUST + REACT</p>
      <h1>{title}</h1>
      <p>{description}</p>
      <code>px make:model Post --all</code>
    </main>
  );
}
"#
    .to_owned()
}

fn styles_template() -> String {
    r":root {
  font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  color: #172033;
  background: #f5f7fb;
}
* { box-sizing: border-box; }
body { margin: 0; min-width: 320px; min-height: 100vh; }
.welcome { width: min(760px, calc(100% - 40px)); margin: 16vh auto 0; }
.eyebrow { color: #315bd6; font-size: 12px; font-weight: 800; letter-spacing: 0.14em; }
h1 { margin: 12px 0; font-size: clamp(42px, 8vw, 76px); line-height: 0.98; }
.welcome > p:not(.eyebrow) { max-width: 640px; color: #5d6879; font-size: 18px; line-height: 1.7; }
code { display: inline-block; margin-top: 18px; padding: 12px 14px; border: 1px solid #d7dce5; background: white; }
".to_owned()
}

fn generated_contracts_template() -> String {
    r#"// Generated by Phoenix. Vite will refresh this file from Rust contracts.
export interface HomeProps {
  title: string;
  description: string;
}
export interface PhoenixPageProps { home: HomeProps }
export type PhoenixSharedProps = Record<string, never>;
export const contractHash = "scaffold" as const;
"#
    .to_owned()
}

fn generated_routes_template() -> String {
    r#"// Generated by Phoenix. Vite will refresh this file from Rust routes.
export const routes = { home: "home" } as const;
export type PhoenixRouteName = "home";
export const home = routes.home;
"#
    .to_owned()
}

fn vite_template(renderer: bool) -> String {
    format!(
        "import {{ defineConfig }} from \"vite\";\nimport {{ phoenix }} from \"@phoenix/vite\";\n\nexport default defineConfig({{\n  plugins: [phoenix({renderer})],\n}});\n",
        renderer = if renderer { "{ renderer: true }" } else { "" },
    )
}

fn tsconfig_template() -> String {
    r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noEmit": true,
    "preserveSymlinks": true,
    "skipLibCheck": true,
    "types": ["vite/client"]
  },
  "include": ["views/**/*.ts", "views/**/*.tsx", "vite.config.ts", "vite.ssr.config.ts"]
}
"#
    .to_owned()
}

fn managed_modules(entries: &[&str]) -> String {
    let body = entries.join("\n");
    if body.is_empty() {
        format!("{MODULES_START}\n{MODULES_END}\n")
    } else {
        format!("{MODULES_START}\n{body}\n{MODULES_END}\n")
    }
}

fn add_rust_item(
    editor: &mut ProjectEditor,
    base: &str,
    name: &QualifiedName,
    content: &str,
) -> Result<(), ScaffoldError> {
    let mut directory = PathBuf::from(base);
    let mut parent_module = directory.join("mod.rs");
    for namespace in &name.modules {
        let module = snake_case(namespace);
        editor.update_managed_lines(
            &parent_module,
            MODULES_START,
            MODULES_END,
            &[format!("pub mod {module};")],
        )?;
        directory.push(&module);
        parent_module = directory.join("mod.rs");
    }
    let module = snake_case(&name.class);
    editor.create(directory.join(format!("{module}.rs")), content.to_owned())?;
    editor.update_managed_lines(
        &parent_module,
        MODULES_START,
        MODULES_END,
        &[
            format!("pub mod {module};"),
            format!("pub use {module}::{};", name.class),
        ],
    )?;
    Ok(())
}

fn add_command(editor: &mut ProjectEditor, name: &QualifiedName) -> Result<(), ScaffoldError> {
    let function_name = snake_case(&name.class);
    if matches!(function_name.as_str(), "serve" | "help") {
        return Err(ScaffoldError::InvalidName(function_name));
    }

    let mut directory = PathBuf::from("app/commands");
    let mut parent_module = directory.join("mod.rs");
    let mut export_path = Vec::new();
    for namespace in &name.modules {
        let module = snake_case(namespace);
        editor.update_managed_lines(
            &parent_module,
            MODULES_START,
            MODULES_END,
            &[format!("pub mod {module};")],
        )?;
        directory.push(&module);
        parent_module = directory.join("mod.rs");
        export_path.push(module);
    }

    editor.create(
        directory.join(format!("{function_name}.rs")),
        command_template(&function_name),
    )?;
    editor.update_managed_lines(
        &parent_module,
        MODULES_START,
        MODULES_END,
        &[
            format!("pub mod {function_name};"),
            format!("pub use {function_name}::{function_name};"),
        ],
    )?;

    if !export_path.is_empty() {
        let path = format!("{}::{function_name}", export_path.join("::"));
        editor.update_managed_lines(
            "app/commands/mod.rs",
            MODULES_START,
            MODULES_END,
            &[format!("pub use {path};")],
        )?;
    }

    editor.update_managed_lines(
        "app/commands/mod.rs",
        COMMANDS_START,
        COMMANDS_END,
        &[format!("{function_name},")],
    )?;
    Ok(())
}

fn add_model(editor: &mut ProjectEditor, model: &QualifiedName) -> Result<(), ScaffoldError> {
    add_rust_item(editor, "app/models", model, &model_template(&model.class))?;
    let path = if model.modules.is_empty() {
        model.class.clone()
    } else {
        format!(
            "{}::{}",
            model
                .modules
                .iter()
                .map(|part| snake_case(part))
                .collect::<Vec<_>>()
                .join("::"),
            model.class
        )
    };
    editor.update_registry(
        "app/models/mod.rs",
        MODELS_START,
        MODELS_END,
        "model",
        &path,
        render_model_registry,
    )
}

fn add_model_migration(
    editor: &mut ProjectEditor,
    model: &QualifiedName,
) -> Result<(), ScaffoldError> {
    let table = pluralize(&snake_case(&model.class));
    add_migration(editor, &format!("create_{table}_table"), &table)
}

fn add_migration(editor: &mut ProjectEditor, name: &str, table: &str) -> Result<(), ScaffoldError> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ScaffoldError::InvalidClock)?
        .as_millis();
    let id = milliseconds.to_string();
    let module = format!("m_{id}_{name}");
    editor.create(
        format!("database/migrations/{module}.rs"),
        migration_template(&id, name, table),
    )?;
    editor.update_registry(
        "database/migrations/mod.rs",
        MIGRATIONS_START,
        MIGRATIONS_END,
        "migration",
        &module,
        render_migration_registry,
    )
}

fn add_controller_route(
    editor: &mut ProjectEditor,
    controller: &QualifiedName,
    resource: bool,
    action: Option<(&QualifiedName, &QualifiedName)>,
) -> Result<(), ScaffoldError> {
    let base = controller
        .class
        .strip_suffix("Controller")
        .unwrap_or(&controller.class);
    let plural = pluralize(&snake_case(base));
    let namespace_modules = controller
        .modules
        .iter()
        .map(|part| snake_case(part))
        .collect::<Vec<_>>();
    let route_file = if namespace_modules.is_empty() {
        plural.clone()
    } else {
        format!("{}_{}", namespace_modules.join("_"), plural)
    };
    let import = if namespace_modules.is_empty() {
        format!("crate::controllers::{}", controller.class)
    } else {
        format!(
            "crate::controllers::{}::{}",
            namespace_modules.join("::"),
            controller.class
        )
    };
    let route_name = if namespace_modules.is_empty() {
        plural.clone()
    } else {
        format!("{}.{}", namespace_modules.join("."), plural)
    };
    let path = if namespace_modules.is_empty() {
        format!("/{}", plural.replace('_', "-"))
    } else {
        format!(
            "/{}/{}",
            namespace_modules
                .iter()
                .map(|part| kebab_case(part))
                .collect::<Vec<_>>()
                .join("/"),
            plural.replace('_', "-")
        )
    };
    editor.create(
        format!("routes/{route_file}.rs"),
        controller_route_template(
            &import,
            &route_name,
            &path,
            &controller.class,
            resource,
            action,
        ),
    )
}

fn add_page(editor: &mut ProjectEditor, page: &PageName) -> Result<(), ScaffoldError> {
    let props = page_props_name(page);
    add_rust_item(
        editor,
        "app/props",
        &props,
        &page_props_template(&props.class, &page.route),
    )?;
    let mut path = PathBuf::from("views/pages");
    for part in &page.parts[..page.parts.len() - 1] {
        path.push(kebab_case(part));
    }
    path.push(format!(
        "{}.tsx",
        kebab_case(page.parts.last().expect("page has one part"))
    ));
    editor.create(
        path,
        page_template(&page.class, &props.class, page.parts.len()),
    )
}

fn page_props_name(page: &PageName) -> QualifiedName {
    QualifiedName {
        modules: page.parts[..page.parts.len() - 1].to_vec(),
        class: format!("{}Props", page.class),
    }
}

fn model_template(name: &str) -> String {
    format!(
        r"use phoenix::database::Model;

#[derive(Debug, Model)]
pub struct {name} {{
    #[key]
    #[auto]
    pub id: u64,
    pub name: String,
}}
"
    )
}

fn request_template(name: &str) -> String {
    format!(
        r#"use phoenix::prelude::{{Validate, ValidationErrors, Validator, max_length, required, rules, string}};
use serde::Deserialize;

#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct {name} {{
    pub name: String,
}}

impl Validate for {name} {{
    fn validate(&self) -> Result<(), ValidationErrors> {{
        let data = serde_json::json!({{ "name": self.name }});
        Validator::new(&data)
            .field("name", rules![required(), string(), max_length(255)])
            .validate()
    }}
}}
"#
    )
}

fn resource_template(name: &str) -> String {
    format!(
        r#"use serde::Serialize;

#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct {name} {{
    pub id: String,
    pub name: String,
}}
"#
    )
}

fn controller_template(name: &str, resource: bool) -> String {
    if !resource {
        return format!(
            r#"use phoenix::prelude::{{Request, Response}};

pub struct {name};

impl {name} {{
    #[allow(clippy::unused_async)]
    pub async fn index(_request: Request) -> Response {{
        Response::text("{name}@index")
    }}
}}
"#
        );
    }
    format!(
        r#"use phoenix::prelude::{{Request, Response, StatusCode}};

pub struct {name};

impl {name} {{
    #[allow(clippy::unused_async)]
    pub async fn index(_request: Request) -> Response {{ Response::text("{name}@index") }}

    #[allow(clippy::unused_async)]
    pub async fn create(_request: Request) -> Response {{ Response::text("{name}@create") }}

    #[allow(clippy::unused_async)]
    pub async fn store(_request: Request) -> Response {{
        Response::text("{name}@store").with_status(StatusCode::CREATED)
    }}

    #[allow(clippy::unused_async)]
    pub async fn show(_request: Request) -> Response {{ Response::text("{name}@show") }}

    #[allow(clippy::unused_async)]
    pub async fn edit(_request: Request) -> Response {{ Response::text("{name}@edit") }}

    #[allow(clippy::unused_async)]
    pub async fn update(_request: Request) -> Response {{ Response::text("{name}@update") }}

    #[allow(clippy::unused_async)]
    pub async fn destroy(_request: Request) -> Response {{
        Response::new(StatusCode::NO_CONTENT, phoenix::http::Bytes::new())
    }}
}}
"#
    )
}

fn model_controller_template(
    controller: &QualifiedName,
    request: &QualifiedName,
    resource: &QualifiedName,
    props: &QualifiedName,
    page: &str,
) -> String {
    let request_path = rust_item_path("requests", request);
    let resource_path = rust_item_path("resources", resource);
    let props_path = rust_item_path("props", props);
    let name = &controller.class;
    let title = page
        .split('/')
        .next()
        .map_or_else(|| "Items".to_owned(), pascal_case);
    format!(
        r#"use phoenix::prelude::{{Json, Page, PageResponseError, Request, Response, StatusCode, Validated}};

use {props_path};
use {request_path};
use {resource_path};

pub struct {name};

impl {name} {{
    pub async fn index(request: Request) -> Result<Response, PageResponseError> {{
        Page::new("{page}", {props_class} {{ title: "{title}".to_owned() }})
            .spa()
            .respond_to(&request, None)
    }}

    #[allow(clippy::unused_async)]
    pub async fn create(_request: Request) -> Response {{ Response::text("{name}@create") }}

    #[allow(clippy::unused_async)]
    pub async fn store(
        Validated(Json(input)): Validated<Json<{request_class}>>,
    ) -> (StatusCode, Json<{resource_class}>) {{
        (
            StatusCode::CREATED,
            Json({resource_class} {{ id: "generated".to_owned(), name: input.name }}),
        )
    }}

    #[allow(clippy::unused_async)]
    pub async fn show(_request: Request) -> Response {{ Response::text("{name}@show") }}

    #[allow(clippy::unused_async)]
    pub async fn edit(_request: Request) -> Response {{ Response::text("{name}@edit") }}

    #[allow(clippy::unused_async)]
    pub async fn update(_request: Request) -> Response {{ Response::text("{name}@update") }}

    #[allow(clippy::unused_async)]
    pub async fn destroy(_request: Request) -> Response {{
        Response::new(StatusCode::NO_CONTENT, phoenix::http::Bytes::new())
    }}
}}
"#,
        props_class = props.class,
        request_class = request.class,
        resource_class = resource.class,
    )
}

fn middleware_template(name: &str) -> String {
    format!(
        r"use phoenix::prelude::{{BoxFuture, Middleware, Next, Request, Response}};

pub struct {name};

impl Middleware for {name} {{
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {{
        Box::pin(async move {{
            // Add authorization, request context, or response policy here.
            next.run(request).await
        }})
    }}
}}
"
    )
}

fn migration_template(id: &str, name: &str, table: &str) -> String {
    format!(
        r#"use phoenix::database::Migration;

#[must_use]
pub fn migration() -> Migration {{
    Migration::new("{id}", "{description}")
        .up("CREATE TABLE {table} (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
        .down("DROP TABLE {table}")
}}
"#,
        description = name.replace('_', " "),
    )
}

fn controller_route_template(
    import: &str,
    route_name: &str,
    path: &str,
    controller: &str,
    resource: bool,
    action: Option<(&QualifiedName, &QualifiedName)>,
) -> String {
    if !resource {
        return format!(
            "use phoenix::prelude::Routes;\n\nuse {import};\n\n#[must_use]\npub fn routes() -> Routes {{\n    Routes::new()\n        .get(\"{path}\", {controller}::index)\n        .name(\"{route_name}.index\")\n}}\n"
        );
    }
    let parameter = snake_case(controller.strip_suffix("Controller").unwrap_or(controller));
    let (prelude, action_imports, store, action_binding) = action.map_or_else(
        || {
            (
                "Routes".to_owned(),
                String::new(),
                format!("{controller}::store"),
                String::new(),
            )
        },
        |(input, output)| {
            (
                "Routes, typed".to_owned(),
                format!(
                    "use {};\nuse {};\n",
                    rust_item_path("requests", input),
                    rust_item_path("resources", output),
                ),
                format!("typed({controller}::store)"),
                format!("\n        .action::<{}, {}>()", input.class, output.class),
            )
        },
    );
    format!(
        r#"use phoenix::prelude::{{{prelude}}};

use {import};
{action_imports}

#[must_use]
pub fn routes() -> Routes {{
    let member = "{path}/{{{parameter}}}";
    Routes::new()
        .get("{path}", {controller}::index)
        .name("{route_name}.index")
        .get("{path}/create", {controller}::create)
        .name("{route_name}.create")
        .post("{path}", {store})
        .name("{route_name}.store"){action_binding}
        .get(member, {controller}::show)
        .name("{route_name}.show")
        .get(format!("{{member}}/edit"), {controller}::edit)
        .name("{route_name}.edit")
        .put(member, {controller}::update)
        .name("{route_name}.update")
        .patch(member, {controller}::update)
        .delete(member, {controller}::destroy)
        .name("{route_name}.destroy")
}}
"#
    )
}

fn rust_item_path(category: &str, name: &QualifiedName) -> String {
    if name.modules.is_empty() {
        format!("crate::{category}::{}", name.class)
    } else {
        format!(
            "crate::{category}::{}::{}",
            name.modules
                .iter()
                .map(|part| snake_case(part))
                .collect::<Vec<_>>()
                .join("::"),
            name.class,
        )
    }
}

fn page_props_template(name: &str, route: &str) -> String {
    format!(
        r#"use serde::Serialize;

#[phoenix::contract(page, page = "{route}")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct {name} {{
    pub title: String,
}}
"#
    )
}

fn page_template(component: &str, props: &str, depth: usize) -> String {
    let contracts = format!("{}generated/contracts.js", "../".repeat(depth));
    format!(
        r#"import type {{ {props} }} from "{contracts}";

export default function {component}({{ title }}: {props}) {{
  return (
    <main>
      <h1>{{title}}</h1>
    </main>
  );
}}
"#
    )
}

fn island_template(component: &str) -> String {
    format!(
        r#"import {{ useState }} from "react";

export interface {component}Props {{
  initialCount?: number;
}}

export default function {component}({{ initialCount = 0 }}: {component}Props) {{
  const [count, setCount] = useState(initialCount);
  return <button type="button" onClick={{() => setCount((value) => value + 1)}}>{{count}}</button>;
}}
"#
    )
}

fn render_model_registry(values: &BTreeSet<String>) -> Vec<String> {
    let mut lines = values
        .iter()
        .map(|value| format!("// phoenix:model: {value}"))
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.extend([
        "#[must_use]".to_owned(),
        "pub fn all() -> phoenix::database::ModelSet {".to_owned(),
        "    phoenix::database::models!(".to_owned(),
    ]);
    lines.extend(values.iter().map(|value| format!("        {value},")));
    lines.extend(["    )".to_owned(), "}".to_owned()]);
    lines
}

fn render_migration_registry(values: &BTreeSet<String>) -> Vec<String> {
    let mut lines = Vec::new();
    for value in values {
        lines.push(format!("// phoenix:migration: {value}"));
        lines.push(format!("pub mod {value};"));
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.extend([
        "#[must_use]".to_owned(),
        "pub fn all() -> Vec<phoenix::database::Migration> {".to_owned(),
        "    vec![".to_owned(),
    ]);
    lines.extend(
        values
            .iter()
            .map(|value| format!("        {value}::migration(),")),
    );
    lines.extend(["    ]".to_owned(), "}".to_owned()]);
    lines
}

struct ProjectEditor {
    root: PathBuf,
    force: bool,
    changes: BTreeMap<PathBuf, String>,
}

impl ProjectEditor {
    fn new(root: &Path, force: bool) -> Self {
        Self {
            root: root.to_path_buf(),
            force,
            changes: BTreeMap::new(),
        }
    }

    fn create(
        &mut self,
        relative: impl Into<PathBuf>,
        content: String,
    ) -> Result<(), ScaffoldError> {
        let relative = safe_relative(relative.into())?;
        let absolute = self.root.join(&relative);
        if !self.force && (absolute.exists() || self.changes.contains_key(&relative)) {
            return Err(ScaffoldError::AlreadyExists(absolute));
        }
        self.changes.insert(relative, content);
        Ok(())
    }

    fn read(&self, relative: &Path) -> Result<String, ScaffoldError> {
        if let Some(content) = self.changes.get(relative) {
            return Ok(content.clone());
        }
        let absolute = self.root.join(relative);
        match fs::read_to_string(&absolute) {
            Ok(content) => Ok(content),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(String::new()),
            Err(source) => Err(ScaffoldError::Io {
                path: absolute,
                source,
            }),
        }
    }

    fn update_managed_lines(
        &mut self,
        relative: impl AsRef<Path>,
        start: &str,
        end: &str,
        added: &[String],
    ) -> Result<(), ScaffoldError> {
        let relative = safe_relative(relative.as_ref().to_path_buf())?;
        let existing = self.read(&relative)?;
        let initialized = if existing.is_empty() {
            format!("{start}\n{end}\n")
        } else {
            existing
        };
        let (before, managed, after) = managed_parts(&initialized, start, end)
            .ok_or_else(|| ScaffoldError::InvalidManagedFile(self.root.join(&relative)))?;
        let mut lines = managed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        lines.extend(added.iter().cloned());
        let body = lines.into_iter().collect::<Vec<_>>().join("\n");
        let body = if body.is_empty() {
            body
        } else {
            format!("{body}\n")
        };
        self.changes
            .insert(relative, format!("{before}{start}\n{body}{end}{after}"));
        Ok(())
    }

    fn update_registry(
        &mut self,
        relative: impl AsRef<Path>,
        start: &str,
        end: &str,
        key: &str,
        value: &str,
        render: fn(&BTreeSet<String>) -> Vec<String>,
    ) -> Result<(), ScaffoldError> {
        let relative = safe_relative(relative.as_ref().to_path_buf())?;
        let existing = self.read(&relative)?;
        let initialized = if existing.is_empty() {
            format!("{start}\n{end}\n")
        } else {
            existing
        };
        let (before, managed, after) = managed_parts(&initialized, start, end)
            .ok_or_else(|| ScaffoldError::InvalidManagedFile(self.root.join(&relative)))?;
        let prefix = format!("// phoenix:{key}: ");
        let mut values = managed
            .lines()
            .filter_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned))
            .collect::<BTreeSet<_>>();
        values.insert(value.to_owned());
        let rendered = render(&values).join("\n");
        let body = if rendered.is_empty() {
            rendered
        } else {
            format!("{rendered}\n")
        };
        self.changes
            .insert(relative, format!("{before}{start}\n{body}{end}{after}"));
        Ok(())
    }

    fn commit(self) -> Result<Vec<PathBuf>, ScaffoldError> {
        let mut written = Vec::with_capacity(self.changes.len());
        for (relative, content) in self.changes {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| ScaffoldError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&path, content).map_err(|source| ScaffoldError::Io {
                path: path.clone(),
                source,
            })?;
            written.push(path);
        }
        Ok(written)
    }
}

fn managed_parts<'a>(
    content: &'a str,
    start: &str,
    end: &str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let start_index = content.find(start)?;
    let managed_start = start_index + start.len();
    let end_relative = content[managed_start..].find(end)?;
    let end_index = managed_start + end_relative;
    if content[end_index + end.len()..].contains(end) || content[..start_index].contains(start) {
        return None;
    }
    Some((
        &content[..start_index],
        content[managed_start..end_index].trim_matches('\n'),
        &content[end_index + end.len()..],
    ))
}

#[derive(Clone, Debug)]
struct QualifiedName {
    modules: Vec<String>,
    class: String,
}

impl QualifiedName {
    fn parse(value: &str) -> Result<Self, ScaffoldError> {
        let parts = name_parts(value)?;
        let class = pascal_case(parts.last().expect("validated names have a leaf"));
        let modules = parts[..parts.len() - 1]
            .iter()
            .map(|part| pascal_case(part))
            .collect();
        Ok(Self { modules, class })
    }

    fn parse_with_suffix(value: &str, suffix: &str) -> Result<Self, ScaffoldError> {
        let mut name = Self::parse(value)?;
        if !name.class.ends_with(suffix) {
            name.class.push_str(suffix);
        }
        Ok(name)
    }

    fn with_leaf(&self, class: String) -> Self {
        Self {
            modules: self.modules.clone(),
            class,
        }
    }

    fn index_page_name(&self) -> PageName {
        let mut parts = self.modules.clone();
        parts.push(pluralize(&self.class));
        parts.push("Index".to_owned());
        PageName::from_parts(parts)
    }
}

#[derive(Clone, Debug)]
struct PageName {
    parts: Vec<String>,
    route: String,
    class: String,
}

impl PageName {
    fn parse(value: &str) -> Result<Self, ScaffoldError> {
        let parts = name_parts(value)?
            .into_iter()
            .map(|part| pascal_case(&part))
            .collect();
        Ok(Self::from_parts(parts))
    }

    fn from_parts(parts: Vec<String>) -> Self {
        let route = parts
            .iter()
            .map(|part| kebab_case(part))
            .collect::<Vec<_>>()
            .join("/");
        let class = parts.iter().map(String::as_str).collect::<String>();
        Self {
            parts,
            route,
            class,
        }
    }
}

fn name_parts(value: &str) -> Result<Vec<String>, ScaffoldError> {
    let normalized = value.replace("::", "/").replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parts.is_empty()
        || parts.iter().any(|part| {
            !part.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            }) || !part
                .chars()
                .any(|character| character.is_ascii_alphabetic())
        })
    {
        return Err(ScaffoldError::InvalidName(value.to_owned()));
    }
    Ok(parts)
}

fn package_name(value: &str) -> Result<String, ScaffoldError> {
    let value = kebab_case(value).trim_matches('-').to_owned();
    if value.is_empty()
        || value.starts_with(|character: char| character.is_ascii_digit())
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(ScaffoldError::InvalidName(value));
    }
    Ok(value)
}

fn snake_identifier(value: &str) -> Result<String, ScaffoldError> {
    let parts = name_parts(value)?;
    Ok(parts
        .iter()
        .map(|part| snake_case(part))
        .collect::<Vec<_>>()
        .join("_"))
}

fn pascal_case(value: &str) -> String {
    words(value)
        .into_iter()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    characters.as_str().to_ascii_lowercase()
                )
            })
        })
        .collect()
}

fn snake_case(value: &str) -> String {
    words(value).join("_").to_ascii_lowercase()
}

fn kebab_case(value: &str) -> String {
    words(value).join("-").to_ascii_lowercase()
}

fn words(value: &str) -> Vec<String> {
    let mut output = String::new();
    let mut previous_lower_or_digit = false;
    for character in value.chars() {
        if character == '-' || character == '_' || character.is_ascii_whitespace() {
            if !output.ends_with('_') && !output.is_empty() {
                output.push('_');
            }
            previous_lower_or_digit = false;
        } else {
            if character.is_ascii_uppercase() && previous_lower_or_digit {
                output.push('_');
            }
            output.push(character);
            previous_lower_or_digit = character.is_ascii_lowercase() || character.is_ascii_digit();
        }
    }
    output
        .split('_')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn pluralize(value: &str) -> String {
    if let Some(stem) = value.strip_suffix('y')
        && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        return format!("{stem}ies");
    }
    if value.ends_with(['s', 'x', 'z']) || value.ends_with("ch") || value.ends_with("sh") {
        format!("{value}es")
    } else {
        format!("{value}s")
    }
}

fn inferred_table(migration: &str) -> &str {
    migration
        .strip_prefix("create_")
        .and_then(|name| name.strip_suffix("_table"))
        .unwrap_or(migration)
}

fn safe_relative(path: PathBuf) -> Result<PathBuf, ScaffoldError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ScaffoldError::InvalidName(path.display().to_string()));
    }
    Ok(path)
}

fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf, ScaffoldError> {
    let path = path.as_ref();
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|current| current.join(path))
        .map_err(|source| ScaffoldError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn ensure_empty_target(path: &Path) -> Result<(), ScaffoldError> {
    match fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(ScaffoldError::ProjectNotEmpty(path.to_path_buf()))
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ScaffoldError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

fn run_optional(program: &'static str, args: &[&str], cwd: &Path) -> Result<(), ScaffoldError> {
    match Command::new(program).args(args).current_dir(cwd).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(ScaffoldError::CommandFailed { program }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ScaffoldError::Io {
            path: cwd.to_path_buf(),
            source,
        }),
    }
}

fn run_required(program: &'static str, args: &[&str], cwd: &Path) -> Result<(), ScaffoldError> {
    match Command::new(program).args(args).current_dir(cwd).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(ScaffoldError::CommandFailed { program }),
        Err(source) => Err(ScaffoldError::Io {
            path: cwd.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(label: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("phoenix-cli-{label}-{}-{id}", std::process::id()))
    }

    fn framework_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn creates_a_complete_local_project_without_installing() {
        let root = temporary_directory("new");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();

        assert!(root.join("src/main.rs").is_file());
        assert!(root.join("src/bin/phoenix-manage.rs").is_file());
        assert!(root.join("config/app.toml").is_file());
        assert!(root.join("config/database.toml").is_file());
        assert!(
            root.join("config/schemas/phoenix-config-database.schema.json")
                .is_file()
        );
        assert!(root.join("taplo.toml").is_file());
        assert!(
            fs::read_to_string(root.join("config/database.toml"))
                .unwrap()
                .contains("connections.mysql")
        );
        assert!(root.join("app/commands/mod.rs").is_file());
        assert!(root.join("config/mod.rs").is_file());
        assert!(root.join("database/seeders/mod.rs").is_file());
        assert!(root.join("routes/web.rs").is_file());
        assert!(root.join("views/pages/home.tsx").is_file());
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("crates/phoenix"));
        assert!(manifest.contains("default-run = \"phoenix-cli-new-"));
        assert!(
            fs::read_to_string(root.join("package.json"))
                .unwrap()
                .contains("file:")
        );
        let main = fs::read_to_string(root.join("src/main.rs")).unwrap();
        assert!(main.contains("Console::new"));
        assert!(main.contains("commands::registry()"));
        assert!(main.contains(".serve("));
        let commands = fs::read_to_string(root.join("app/commands/mod.rs")).unwrap();
        assert!(commands.contains("commands!"));
        assert!(commands.contains("<phoenix:commands>"));
        let application = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(application.contains("pub mod commands"));
        assert!(application.contains("NonceSecurityPolicy::development"));
        assert!(application.contains("with_middleware(content_security_policy(config))"));
        assert!(application.contains("with_middleware(RequestId)"));
        assert!(application.contains("with_middleware(AccessLog)"));
        assert!(application.contains("SessionMiddleware::new"));
        assert!(application.contains("with_middleware(Csrf)"));
        assert!(application.contains("TrustedProxies::new"));
        assert!(application.contains("HostAllowlist::new"));
        assert!(application.contains("RateLimit::new"));
        assert!(application.contains("StateMiddleware::new(config.clone())"));
        let config = fs::read_to_string(root.join("config/mod.rs")).unwrap();
        assert!(config.contains("AppConfig::load()"));
        assert!(config.lines().count() < 20);
        let manager = fs::read_to_string(root.join("src/bin/phoenix-manage.rs")).unwrap();
        assert!(manager.contains("MigrationRunner::new"));
        assert!(manager.contains("migrations::all()"));
        assert!(manager.contains("seeders::run"));
        let models = fs::read_to_string(root.join("app/models/mod.rs")).unwrap();
        let migrations = fs::read_to_string(root.join("database/migrations/mod.rs")).unwrap();
        assert!(models.contains("pub fn all()"));
        assert!(migrations.contains("pub fn all()"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn make_command_registers_async_handler() {
        let root = temporary_directory("command");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();
        generator
            .command("Update", GenerateOptions::default())
            .unwrap();

        assert!(root.join("app/commands/update.rs").is_file());
        let module = fs::read_to_string(root.join("app/commands/mod.rs")).unwrap();
        assert!(module.contains("pub mod update;"));
        assert!(module.contains("pub use update::update;"));
        assert!(module.contains("update,"));
        let command = fs::read_to_string(root.join("app/commands/update.rs")).unwrap();
        assert!(command.contains("pub async fn update"));
        assert!(command.contains("CommandContext<'_>"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_all_registers_every_supported_business_artifact() {
        let root = temporary_directory("model-all");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();
        generator
            .model(
                "Admin/Post",
                ModelOptions {
                    all: true,
                    ..ModelOptions::default()
                },
            )
            .unwrap();
        generator.model("Comment", ModelOptions::default()).unwrap();

        assert!(root.join("app/models/admin/post.rs").is_file());
        assert!(
            root.join("app/controllers/admin/post_controller.rs")
                .is_file()
        );
        assert!(
            root.join("app/requests/admin/store_post_request.rs")
                .is_file()
        );
        assert!(root.join("app/resources/admin/post_resource.rs").is_file());
        assert!(root.join("routes/admin_posts.rs").is_file());
        assert!(root.join("views/pages/admin/posts/index.tsx").is_file());
        let routes = fs::read_to_string(root.join("routes/admin_posts.rs")).unwrap();
        assert!(routes.contains(".name(\"admin.posts.index\")"));
        assert!(routes.contains(".name(\"admin.posts.destroy\")"));
        assert!(routes.contains("typed(PostController::store)"));
        assert!(routes.contains(".action::<StorePostRequest, PostResource>()"));
        let controller =
            fs::read_to_string(root.join("app/controllers/admin/post_controller.rs")).unwrap();
        assert!(controller.contains("Validated(Json(input))"));
        assert!(controller.contains("Page::new(\"admin/posts/index\""));
        let page = fs::read_to_string(root.join("views/pages/admin/posts/index.tsx")).unwrap();
        assert!(page.contains("../../../generated/contracts.js"));
        let models = fs::read_to_string(root.join("app/models/mod.rs")).unwrap();
        assert!(models.contains("admin::Post"));
        assert!(models.contains("Comment"));
        let migrations = fs::read_to_string(root.join("database/migrations/mod.rs")).unwrap();
        assert!(migrations.contains("pub fn all()"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generators_refuse_overwrites_and_path_traversal() {
        let root = temporary_directory("safety");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();
        generator
            .controller("Report", ControllerOptions::default())
            .unwrap();
        assert!(matches!(
            generator.controller("Report", ControllerOptions::default()),
            Err(ScaffoldError::AlreadyExists(_))
        ));
        assert!(matches!(
            generator.page("../outside", GenerateOptions::default()),
            Err(ScaffoldError::InvalidName(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
