use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    process::Command,
    str::FromStr,
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
const PROJECT_OPTIONS_FILE: &str = ".phoenix";
const PHOENIXRS_VERSION: &str = "0.1.3";
const APIZERO_REACT_VERSION: &str = "0.1.2";
const APIZERO_REACT_SSR_VERSION: &str = "0.1.2";
const APIZERO_VITE_VERSION: &str = "0.1.3";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencySource {
    Registry,
    Local(PathBuf),
}

/// Options for [`ProjectGenerator::update_core`] / `px update`.
#[derive(Clone, Debug)]
pub struct UpdateProjectOptions {
    pub dependencies: DependencySource,
    pub install_dependencies: bool,
    pub dry_run: bool,
}

impl UpdateProjectOptions {
    #[must_use]
    pub fn new() -> Self {
        Self {
            dependencies: DependencySource::discover(),
            install_dependencies: true,
            dry_run: false,
        }
    }

    #[must_use]
    pub fn dependencies(mut self, dependencies: DependencySource) -> Self {
        self.dependencies = dependencies;
        self
    }

    #[must_use]
    pub const fn install_dependencies(mut self, install: bool) -> Self {
        self.install_dependencies = install;
        self
    }

    #[must_use]
    pub const fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }
}

impl Default for UpdateProjectOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredProjectOptions {
    frontend: ProjectFrontend,
    database: Option<ProjectDatabase>,
    render_mode: ProjectRenderMode,
    tailwind: bool,
    features: Vec<ProjectFeature>,
}

/// One optional official Feature selected during `px new --feature ...`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectFeature {
    Captcha,
    Pay,
    Notify,
}

impl ProjectFeature {
    pub const ALL: [Self; 3] = [Self::Captcha, Self::Pay, Self::Notify];

    /// Cargo feature name on the `phoenixrs` facade and `config/<key>.toml` stem.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Captcha => "captcha",
            Self::Pay => "pay",
            Self::Notify => "notify",
        }
    }

    /// One-line Chinese description used by the interactive wizard.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::Captcha => "图形验证码（SVG、一次一用，GET /captcha）",
            Self::Pay => "聚合支付（微信 Native / 支付宝当面付 / Mock）",
            Self::Notify => "通知（邮件 + 数据库双通道）",
        }
    }

    /// Whether this Feature registers database migrations through its plugin.
    #[must_use]
    pub const fn has_migrations(self) -> bool {
        matches!(self, Self::Pay | Self::Notify)
    }
}

impl FromStr for ProjectFeature {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "captcha" => Ok(Self::Captcha),
            "pay" | "payment" | "payments" => Ok(Self::Pay),
            "notify" | "notification" | "notifications" => Ok(Self::Notify),
            _ => Err(format!(
                "unknown feature `{value}`; expected captcha, pay, or notify"
            )),
        }
    }
}

/// Parse a comma-separated `--feature` list; deduplicates and keeps a stable order.
///
/// # Errors
///
/// Returns an error naming the first unknown feature.
pub fn parse_feature_list(value: &str) -> Result<Vec<ProjectFeature>, String> {
    let mut features = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let feature = part.parse::<ProjectFeature>()?;
        if !features.contains(&feature) {
            features.push(feature);
        }
    }
    features.sort_unstable();
    Ok(features)
}

/// Initial React rendering mode selected by `px new`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectRenderMode {
    Spa,
    Ssr,
    #[default]
    Islands,
}

impl ProjectRenderMode {
    #[must_use]
    pub const fn page_builder(self) -> &'static str {
        match self {
            Self::Spa => ".spa()",
            Self::Ssr => ".ssr()",
            Self::Islands => ".islands()",
        }
    }
}

impl FromStr for ProjectRenderMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "spa" => Ok(Self::Spa),
            "ssr" => Ok(Self::Ssr),
            "islands" | "island" => Ok(Self::Islands),
            _ => Err("render mode must be spa, ssr, or islands".to_owned()),
        }
    }
}

/// One optional database driver selected when a project is created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectDatabase {
    Sqlite,
    Pgsql,
    Mysql,
    All,
}

/// Source format used for generated React components.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectFrontend {
    Jsx,
    #[default]
    Tsx,
}

impl ProjectFrontend {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jsx => "jsx",
            Self::Tsx => "tsx",
        }
    }
}

impl FromStr for ProjectFrontend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "jsx" | "js" => Ok(Self::Jsx),
            "tsx" | "ts" => Ok(Self::Tsx),
            _ => Err("frontend must be jsx or tsx".to_owned()),
        }
    }
}

impl ProjectDatabase {
    #[must_use]
    pub const fn cargo_feature(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Pgsql => "pgsql",
            Self::Mysql => "mysql",
            Self::All => "all-databases",
        }
    }

    #[must_use]
    pub const fn toasty_features(self) -> &'static str {
        match self {
            Self::Sqlite => "serde\", \"sqlite",
            Self::Pgsql => "postgresql\", \"serde",
            Self::Mysql => "mysql\", \"serde",
            Self::All => "mysql\", \"postgresql\", \"serde\", \"sqlite",
        }
    }
}

impl FromStr for ProjectDatabase {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Ok(Self::Sqlite),
            "pgsql" | "postgres" | "postgresql" => Ok(Self::Pgsql),
            "mysql" | "mariadb" => Ok(Self::Mysql),
            "all" => Ok(Self::All),
            _ => Err("database must be sqlite, pgsql, mysql, or all".to_owned()),
        }
    }
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
    pub render_mode: ProjectRenderMode,
    pub database: Option<ProjectDatabase>,
    pub frontend: ProjectFrontend,
    pub tailwind: bool,
    pub features: Vec<ProjectFeature>,
}

impl NewProjectOptions {
    #[must_use]
    pub fn new(target: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            dependencies: DependencySource::discover(),
            initialize_git: false,
            install_dependencies: true,
            render_mode: ProjectRenderMode::default(),
            database: None,
            frontend: ProjectFrontend::default(),
            tailwind: false,
            features: Vec::new(),
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

    #[must_use]
    pub const fn render_mode(mut self, render_mode: ProjectRenderMode) -> Self {
        self.render_mode = render_mode;
        self
    }

    #[must_use]
    pub const fn database(mut self, database: Option<ProjectDatabase>) -> Self {
        self.database = database;
        self
    }

    #[must_use]
    pub const fn frontend(mut self, frontend: ProjectFrontend) -> Self {
        self.frontend = frontend;
        self
    }

    #[must_use]
    pub const fn tailwind(mut self, tailwind: bool) -> Self {
        self.tailwind = tailwind;
        self
    }

    #[must_use]
    pub fn features(mut self, features: Vec<ProjectFeature>) -> Self {
        self.features = features;
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

#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModelOptions {
    pub all: bool,
    pub api_resource: bool,
    pub controller: bool,
    pub factory: bool,
    pub force: bool,
    pub migration: bool,
    pub page: bool,
    pub relations: Vec<Relation>,
    pub request: bool,
    pub resource_controller: bool,
}

/// How a generated model relates to another one.
///
/// The whole point of naming these is that picking the *kind* and the *target*
/// is all the information a relation needs — the foreign key, the column it
/// maps to, and the pairing are conventions `#[phoenix::model]` fills in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    /// This model holds the foreign key (`Post` → `User`).
    BelongsTo,
    /// The other model holds the foreign key, and there are many (`User` → `Post`).
    HasMany,
    /// The other model holds the foreign key, and there is at most one.
    HasOne,
}

impl RelationKind {
    /// The attribute this kind writes.
    const fn attribute(self) -> &'static str {
        match self {
            Self::BelongsTo => "belongs_to",
            Self::HasMany => "has_many",
            Self::HasOne => "has_one",
        }
    }

    /// The CLI flag that selects it.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        match self {
            Self::BelongsTo => "--belongs-to",
            Self::HasMany => "--has-many",
            Self::HasOne => "--has-one",
        }
    }
}

/// One relation declared on the command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Relation {
    /// Which side of the foreign key this model is on.
    pub kind: RelationKind,
    /// The model being related to, exactly as typed (`User`, `blog::Post`).
    pub target: String,
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
    #[error(
        "make:auth needs a database for the persistent User model; recreate with \
         `px new <app> --database sqlite` (or pgsql/mysql) and try again"
    )]
    AuthRequiresDatabase,
}

/// Write the project skeleton only (no `git init`, no `npm install`).
///
/// Returns the number of files written so callers can report progress.
///
/// # Errors
///
/// Returns an error for invalid names, non-empty targets, invalid local
/// framework paths, or file-system failures.
pub fn scaffold_project(options: &NewProjectOptions) -> Result<usize, ScaffoldError> {
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
    for (path, content) in project_files(&package, options)? {
        editor.create(path, content)?;
    }
    Ok(editor.commit()?.len())
}

/// Create a complete Phoenix application that can immediately run `px dev`.
///
/// # Errors
///
/// Returns an error for invalid names, non-empty targets, invalid local framework
/// paths, file-system failures, or dependency/bootstrap command failures.
pub fn create_project(options: &NewProjectOptions) -> Result<(), ScaffoldError> {
    scaffold_project(options)?;
    let target = absolute_path(&options.target)?;
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
        if !self.root.join("node_modules/@apizero/vite").is_dir() {
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
            options.factory = true;
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
        add_model(&mut editor, &model, &options.relations)?;
        if options.factory {
            add_factory(&mut editor, &model, &options.relations)?;
        }
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
            add_page(&mut editor, &page, self.frontend())?;
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
        add_page(&mut editor, &page, self.frontend())?;
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
        let frontend = self.frontend();
        path.push(format!(
            "{}.{}",
            kebab_case(&name.class),
            frontend.extension()
        ));
        editor.create(path, island_template(&name.class, frontend))?;
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

    /// Generate the authentication starter kit (`px make:auth`).
    ///
    /// Creates named login / logout / register / password-reset routes with a
    /// stricter rate limit, an `AuthController`, validated request contracts,
    /// browser-safe resources, page props, and React form pages.
    ///
    /// # Errors
    ///
    /// Returns an error for conflicts or malformed managed files.
    pub fn auth(&self, options: GenerateOptions) -> Result<Vec<PathBuf>, ScaffoldError> {
        // The generated slice authenticates against a persistent `users` table,
        // so a database driver must have been selected at `px new`.
        if self.stored_options().database.is_none() {
            return Err(ScaffoldError::AuthRequiresDatabase);
        }
        let frontend = self.frontend();
        let mut editor = ProjectEditor::new(&self.root, options.force);
        // Persistent User model + migration; registered like `px make:model`.
        add_rust_item(
            &mut editor,
            "app/models",
            &QualifiedName {
                modules: Vec::new(),
                class: "User".to_owned(),
            },
            &auth_user_model_template(),
        )?;
        editor.update_registry(
            "app/models/mod.rs",
            MODELS_START,
            MODELS_END,
            "model",
            "User",
            render_model_registry,
        )?;
        add_auth_user_migration(&mut editor)?;
        // Credentials use Argon2id (`password`) and the guard lives in the
        // `phoenix-auth` crate (`auth`); enable both facade features.
        enable_project_features(&mut editor, &["password", "auth"])?;
        for (class, content) in [
            ("LoginInput", login_input_template()),
            ("RegisterInput", register_input_template()),
            ("PasswordResetInput", password_reset_input_template()),
        ] {
            add_rust_item(
                &mut editor,
                "app/requests",
                &QualifiedName {
                    modules: Vec::new(),
                    class: class.to_owned(),
                },
                &content,
            )?;
        }
        for (class, content) in [
            ("AuthSessionResource", auth_session_resource_template()),
            ("AuthMessageResource", auth_message_resource_template()),
        ] {
            add_rust_item(
                &mut editor,
                "app/resources",
                &QualifiedName {
                    modules: Vec::new(),
                    class: class.to_owned(),
                },
                &content,
            )?;
        }
        add_rust_item(
            &mut editor,
            "app/controllers",
            &QualifiedName {
                modules: Vec::new(),
                class: "AuthController".to_owned(),
            },
            &auth_controller_template(),
        )?;
        for (page, content) in [
            ("auth/login", auth_login_page_template(frontend)),
            ("auth/register", auth_register_page_template(frontend)),
            (
                "auth/forgot-password",
                auth_forgot_password_page_template(frontend),
            ),
        ] {
            add_auth_page(&mut editor, page, content, frontend)?;
        }
        editor.create("routes/auth.rs", auth_routes_template())?;
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

    fn frontend(&self) -> ProjectFrontend {
        self.stored_options().frontend
    }

    fn stored_options(&self) -> StoredProjectOptions {
        read_stored_options(&self.root)
    }

    /// Refresh framework-owned core files without touching business code.
    ///
    /// Updates `src/lib.rs` / `src/main.rs`, Vite/TS config, config schemas, dependency
    /// pins for `phoenixrs` / `@apizero/*`, and (when the project uses a database)
    /// `src/bin/phoenix-manage.rs`. Leaves controllers, routes, pages, and user TOML alone.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid local framework roots, I/O failures, or npm failures.
    pub fn update_core(
        &self,
        options: &UpdateProjectOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        if let DependencySource::Local(root) = &options.dependencies
            && !is_framework_root(root)
        {
            return Err(ScaffoldError::InvalidFrameworkRoot(root.clone()));
        }

        let package = read_package_name(&self.root)?;
        let crate_name = package.replace('-', "_");
        let stored = self.stored_options();
        let has_database = stored.database.is_some();
        let (rust_dependency, react, react_ssr, vite) =
            framework_dependency_pins(&options.dependencies, &stored.features)?;

        let mut planned: BTreeMap<PathBuf, String> = BTreeMap::new();
        planned.insert("src/main.rs".into(), main_template(&crate_name));
        planned.insert(
            "src/lib.rs".into(),
            lib_template(has_database, &stored.features),
        );
        planned.insert("config/mod.rs".into(), config_template(has_database));
        planned.insert(
            "vite.config.ts".into(),
            vite_template(false, stored.tailwind),
        );
        planned.insert(
            "vite.ssr.config.ts".into(),
            vite_template(true, stored.tailwind),
        );
        planned.insert("tsconfig.json".into(), tsconfig_template());
        planned.insert(
            ".gitignore".into(),
            "/target\n/node_modules\n/public/assets\n/public/ssr\n/views/generated/*.ts\n/dist\n/storage/*.sqlite\n/storage/*.sqlite-*\n.env\n.DS_Store\n".to_owned(),
        );
        planned.insert(PROJECT_OPTIONS_FILE.into(), format_stored_options(&stored));

        if stored.database.is_some() {
            planned.insert(
                "src/bin/phoenix-manage.rs".into(),
                management_template(
                    &crate_name,
                    stored
                        .features
                        .iter()
                        .any(|feature| feature.has_migrations()),
                ),
            );
            // Ensure empty registries exist when upgrading a no-db project that later
            // recorded a database in `.phoenix` — do not overwrite populated registries.
            for (path, content) in [
                ("app/models/mod.rs", empty_model_registry()),
                ("database/migrations/mod.rs", empty_migration_registry()),
                ("database/seeders/mod.rs", seeder_template()),
            ] {
                if !self.root.join(path).is_file() {
                    planned.insert(path.into(), content);
                }
            }
        }

        // Feature config files carry user values: create only when missing.
        for feature in &stored.features {
            let path = format!("config/{}.toml", feature.key());
            if !self.root.join(&path).is_file() {
                planned.insert(path.into(), feature_config_template(*feature));
            }
        }

        let cargo_path = PathBuf::from("Cargo.toml");
        let cargo = fs::read_to_string(self.root.join(&cargo_path)).map_err(|source| {
            ScaffoldError::Io {
                path: self.root.join(&cargo_path),
                source,
            }
        })?;
        let cargo = patch_cargo_toml_core(&cargo, &rust_dependency);
        planned.insert(cargo_path, cargo);

        let package_json_path = PathBuf::from("package.json");
        let package_json =
            fs::read_to_string(self.root.join(&package_json_path)).map_err(|source| {
                ScaffoldError::Io {
                    path: self.root.join(&package_json_path),
                    source,
                }
            })?;
        let package_json =
            patch_package_json_core(&package_json, &react, &react_ssr, &vite, stored.tailwind)?;
        planned.insert(package_json_path, package_json);

        let mut changed = Vec::new();
        for (relative, content) in &planned {
            let absolute = self.root.join(relative);
            let existing = match fs::read_to_string(&absolute) {
                Ok(text) => text,
                Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
                Err(source) => {
                    return Err(ScaffoldError::Io {
                        path: absolute,
                        source,
                    });
                }
            };
            if existing == *content {
                continue;
            }
            changed.push(absolute.clone());
            if options.dry_run {
                continue;
            }
            if let Some(parent) = absolute.parent() {
                fs::create_dir_all(parent).map_err(|source| ScaffoldError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&absolute, content).map_err(|source| ScaffoldError::Io {
                path: absolute,
                source,
            })?;
        }

        if !options.dry_run
            && options.install_dependencies
            && self.root.join("package.json").is_file()
        {
            run_required("npm", &["install"], &self.root)?;
            let _ = self.refresh_types()?;
        }

        Ok(changed)
    }
}

fn is_project_root(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("package.json").is_file()
        && path.join("app").is_dir()
        && path.join("routes").is_dir()
        && path.join("views").is_dir()
}

fn framework_dependency_pins(
    source: &DependencySource,
    features: &[ProjectFeature],
) -> Result<(String, String, String, String), ScaffoldError> {
    // Comma-separated `"captcha", "pay"` fragment for the phoenix dependency line.
    let feature_list = features
        .iter()
        .map(|feature| format!("\"{}\"", feature.key()))
        .collect::<Vec<_>>()
        .join(", ");
    let feature_suffix = if feature_list.is_empty() {
        String::new()
    } else {
        format!(", features = [{feature_list}]")
    };
    match source {
        // Plain semver ranges: the user's own npm registry (npmmirror and
        // friends included) resolves them. Never emit registry.npmjs.org
        // tarball URLs — they bypass configured mirrors.
        DependencySource::Registry => Ok((
            format!(
                "phoenix = {{ package = \"phoenixrs\", version = \"{PHOENIXRS_VERSION}\", default-features = false{feature_suffix} }}"
            ),
            format!("^{APIZERO_REACT_VERSION}"),
            format!("^{APIZERO_REACT_SSR_VERSION}"),
            format!("^{APIZERO_VITE_VERSION}"),
        )),
        DependencySource::Local(root) => {
            let root = absolute_path(root)?;
            Ok((
                format!(
                    "phoenix = {{ package = \"phoenixrs\", path = {}, default-features = false{feature_suffix} }}",
                    json_string(&root.join("crates/phoenix").to_string_lossy())
                ),
                format!("file:{}", root.join("packages/phoenix-react").display()),
                format!("file:{}", root.join("packages/phoenix-react-ssr").display()),
                format!("file:{}", root.join("packages/phoenix-vite").display()),
            ))
        }
    }
}

fn format_stored_options(options: &StoredProjectOptions) -> String {
    let database = options.database.map_or("none", database_option_key);
    let render_mode = match options.render_mode {
        ProjectRenderMode::Spa => "spa",
        ProjectRenderMode::Ssr => "ssr",
        ProjectRenderMode::Islands => "islands",
    };
    let features = options
        .features
        .iter()
        .map(|feature| feature.key())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "frontend={}\nrender_mode={render_mode}\ndatabase={database}\ntailwind={}\nfeatures={features}\n",
        options.frontend.extension(),
        if options.tailwind { "true" } else { "false" },
    )
}

fn database_option_key(database: ProjectDatabase) -> &'static str {
    match database {
        ProjectDatabase::Sqlite => "sqlite",
        ProjectDatabase::Pgsql => "pgsql",
        ProjectDatabase::Mysql => "mysql",
        ProjectDatabase::All => "all",
    }
}

fn read_stored_options(root: &Path) -> StoredProjectOptions {
    let mut options = StoredProjectOptions {
        frontend: ProjectFrontend::default(),
        database: None,
        render_mode: ProjectRenderMode::default(),
        tailwind: false,
        features: Vec::new(),
    };
    let mut render_mode_recorded = false;

    if let Ok(source) = fs::read_to_string(root.join(PROJECT_OPTIONS_FILE)) {
        for line in source.lines() {
            if let Some(value) = line.strip_prefix("frontend=") {
                if let Ok(frontend) = value.parse() {
                    options.frontend = frontend;
                }
            } else if let Some(value) = line.strip_prefix("render_mode=") {
                if let Ok(render_mode) = value.parse() {
                    options.render_mode = render_mode;
                    render_mode_recorded = true;
                }
            } else if let Some(value) = line.strip_prefix("database=") {
                options.database = match value.trim() {
                    "" | "none" | "no" => None,
                    other => other.parse().ok(),
                };
            } else if let Some(value) = line.strip_prefix("tailwind=") {
                options.tailwind = matches!(value.trim(), "1" | "true" | "yes");
            } else if let Some(value) = line.strip_prefix("features=")
                && let Ok(features) = parse_feature_list(value)
            {
                options.features = features;
            }
        }
    }

    if options.database.is_none() && root.join("src/bin/phoenix-manage.rs").is_file() {
        options.database = Some(infer_database(root));
    }
    if !options.tailwind
        && fs::read_to_string(root.join("package.json"))
            .ok()
            .is_some_and(|text| text.contains("tailwindcss"))
    {
        options.tailwind = true;
    }
    if !render_mode_recorded
        && let Ok(controller) = fs::read_to_string(root.join("app/controllers/home_controller.rs"))
        && let Some(render_mode) = sniff_home_render_mode(&controller)
    {
        options.render_mode = render_mode;
    }
    if options.frontend == ProjectFrontend::default()
        && root.join("views/pages/home.jsx").is_file()
        && !root.join("views/pages/home.tsx").is_file()
    {
        options.frontend = ProjectFrontend::Jsx;
    }
    options
}

/// Legacy fallback for projects without `render_mode=` in `.phoenix`: the mode
/// marker closest after `Page::new("home"` wins, so the demo-page methods in
/// the same controller do not confuse the detection.
fn sniff_home_render_mode(controller: &str) -> Option<ProjectRenderMode> {
    let start = controller.find("\"home\"").unwrap_or(0);
    let tail = &controller[start..];
    [
        (".spa()", ProjectRenderMode::Spa),
        (".ssr()", ProjectRenderMode::Ssr),
        (".islands()", ProjectRenderMode::Islands),
    ]
    .into_iter()
    .filter_map(|(marker, mode)| tail.find(marker).map(|index| (index, mode)))
    .min_by_key(|(index, _)| *index)
    .map(|(_, mode)| mode)
}

/// Infer the driver for legacy projects (no `database=` line in `.phoenix`)
/// from the Cargo feature default, falling back to the `.env` `DATABASE_URL`.
fn infer_database(root: &Path) -> ProjectDatabase {
    if let Ok(cargo) = fs::read_to_string(root.join("Cargo.toml")) {
        if cargo.contains("default = [\"all-databases\"]") || cargo.contains("all-databases =") {
            return ProjectDatabase::All;
        }
        if cargo.contains("default = [\"mysql\"]") {
            return ProjectDatabase::Mysql;
        }
        if cargo.contains("default = [\"pgsql\"]") {
            return ProjectDatabase::Pgsql;
        }
        if cargo.contains("default = [\"sqlite\"]") {
            return ProjectDatabase::Sqlite;
        }
    }
    for env_file in [".env", ".env.example"] {
        if let Ok(env) = fs::read_to_string(root.join(env_file)) {
            for line in env.lines() {
                let Some(url) = line.trim().strip_prefix("DATABASE_URL=") else {
                    continue;
                };
                let url = url.trim();
                if url.starts_with("sqlite:") {
                    return ProjectDatabase::Sqlite;
                }
                if url.starts_with("postgres") {
                    return ProjectDatabase::Pgsql;
                }
                if url.starts_with("mysql:") {
                    return ProjectDatabase::Mysql;
                }
            }
        }
    }
    ProjectDatabase::Sqlite
}

fn read_package_name(root: &Path) -> Result<String, ScaffoldError> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path).map_err(|source| ScaffoldError::Io {
        path: path.clone(),
        source,
    })?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("name = ") {
            let value = value.trim();
            if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                return Ok(value[1..value.len() - 1].to_owned());
            }
        }
    }
    Err(ScaffoldError::InvalidManagedFile(path))
}

fn patch_cargo_toml_core(cargo: &str, phoenix_dependency: &str) -> String {
    let mut lines = cargo.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut replaced = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("phoenix =") || trimmed.starts_with("phoenixrs =") {
            phoenix_dependency.clone_into(line);
            replaced = true;
        }
    }
    if !replaced {
        if let Some(index) = lines
            .iter()
            .position(|line| line.trim() == "[dependencies]")
        {
            lines.insert(index + 1, phoenix_dependency.to_owned());
        } else {
            lines.push(String::new());
            lines.push("[dependencies]".to_owned());
            lines.push(phoenix_dependency.to_owned());
        }
    }

    let required_features = [
        ("tls =", "tls = [\"phoenix/tls\"]"),
        ("websocket =", "websocket = [\"phoenix/websocket\"]"),
        ("sse =", "sse = [\"phoenix/sse\"]"),
        ("auth =", "auth = [\"phoenix/auth\"]"),
        ("jwt =", "jwt = [\"phoenix/jwt\"]"),
        ("password =", "password = [\"phoenix/password\"]"),
        ("metrics =", "metrics = [\"phoenix/metrics\"]"),
        ("redis =", "redis = [\"phoenix/redis\"]"),
        ("storage =", "storage = [\"phoenix/storage\"]"),
        ("queue =", "queue = [\"phoenix/queue\"]"),
        ("mail =", "mail = [\"phoenix/mail\"]"),
        ("testing =", "testing = [\"phoenix/testing\"]"),
        ("factory =", "factory = [\"phoenix/factory\"]"),
    ];
    let mut missing = Vec::new();
    for (prefix, line) in required_features {
        if !lines
            .iter()
            .any(|existing| existing.trim_start().starts_with(prefix))
        {
            missing.push(line.to_owned());
        }
    }
    if !missing.is_empty() {
        let insert_at = lines
            .iter()
            .position(|line| line.trim() == "[dependencies]")
            .unwrap_or(lines.len());
        for (offset, line) in missing.into_iter().enumerate() {
            lines.insert(insert_at + offset, line);
        }
    }

    let mut body = lines.join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

fn patch_package_json_core(
    raw: &str,
    react: &str,
    react_ssr: &str,
    vite: &str,
    tailwind: bool,
) -> Result<String, ScaffoldError> {
    let mut value: serde_json::Value =
        serde_json::from_str(raw).map_err(|source| ScaffoldError::Io {
            path: PathBuf::from("package.json"),
            source: std::io::Error::new(ErrorKind::InvalidData, source.to_string()),
        })?;
    let object = value.as_object_mut().ok_or_else(|| ScaffoldError::Io {
        path: PathBuf::from("package.json"),
        source: std::io::Error::new(
            ErrorKind::InvalidData,
            "package.json root must be an object",
        ),
    })?;

    let dependencies = object
        .entry("dependencies")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(deps) = dependencies.as_object_mut() {
        deps.insert(
            "@apizero/react".to_owned(),
            serde_json::Value::String(react.to_owned()),
        );
        deps.insert(
            "@apizero/react-ssr".to_owned(),
            serde_json::Value::String(react_ssr.to_owned()),
        );
        deps.insert(
            "@apizero/vite".to_owned(),
            serde_json::Value::String(vite.to_owned()),
        );
    }

    let scripts = object
        .entry("scripts")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(scripts) = scripts.as_object_mut() {
        scripts.insert(
            "dev".to_owned(),
            serde_json::Value::String("vite --host 127.0.0.1".to_owned()),
        );
        scripts.insert(
            "build".to_owned(),
            serde_json::Value::String("npm run build:client && npm run build:ssr".to_owned()),
        );
        scripts.insert(
            "build:client".to_owned(),
            serde_json::Value::String("vite build".to_owned()),
        );
        scripts.insert(
            "build:ssr".to_owned(),
            serde_json::Value::String("vite build --config vite.ssr.config.ts".to_owned()),
        );
        scripts.insert(
            "types".to_owned(),
            serde_json::Value::String(
                "node -e \"import('@apizero/vite').then(({ generateRouteTypes }) => generateRouteTypes('routes', 'views/generated/routes.ts', '.', 'views/generated/contracts.ts'))\""
                    .to_owned(),
            ),
        );
        scripts.insert(
            "typecheck".to_owned(),
            serde_json::Value::String("npm run types && tsc --noEmit".to_owned()),
        );
    }

    if tailwind {
        let dev_dependencies = object
            .entry("devDependencies")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(deps) = dev_dependencies.as_object_mut() {
            deps.entry("@tailwindcss/vite".to_owned())
                .or_insert_with(|| serde_json::Value::String("^4.3.0".to_owned()));
            deps.entry("tailwindcss".to_owned())
                .or_insert_with(|| serde_json::Value::String("^4.3.0".to_owned()));
        }
    }

    let mut rendered =
        serde_json::to_string_pretty(&value).map_err(|source| ScaffoldError::Io {
            path: PathBuf::from("package.json"),
            source: std::io::Error::new(ErrorKind::InvalidData, source.to_string()),
        })?;
    rendered.push('\n');
    Ok(rendered)
}

// The body is the flat scaffold file list; the length is data, not logic.
#[allow(clippy::too_many_lines)]
fn project_files(
    package: &str,
    options: &NewProjectOptions,
) -> Result<Vec<(PathBuf, String)>, ScaffoldError> {
    let (rust_dependency, react, react_ssr, vite) =
        framework_dependency_pins(&options.dependencies, &options.features)?;
    let crate_name = package.replace('-', "_");
    let tailwind = if options.tailwind {
        ",\n    \"@tailwindcss/vite\": \"^4.3.0\",\n    \"tailwindcss\": \"^4.3.0\""
    } else {
        ""
    };
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
    "types": "node -e \"import('@apizero/vite').then(({{ generateRouteTypes }}) => generateRouteTypes('routes', 'views/generated/routes.ts', '.', 'views/generated/contracts.ts'))\"",
    "typecheck": "npm run types && tsc --noEmit"
  }},
  "dependencies": {{
    "@apizero/react": {react},
    "@apizero/react-ssr": {react_ssr},
    "@apizero/vite": {vite},
    "react": "^19.1.0",
    "react-dom": "^19.1.0"
  }},
  "devDependencies": {{
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "typescript": "^5.8.0",
    "vite": "^7.3.6"{tailwind}
  }}
}}
"#,
        package = json_string(package),
        react = json_string(&react),
        react_ssr = json_string(&react_ssr),
        vite = json_string(&vite),
        tailwind = tailwind,
    );
    let (database_features, toasty_dependency) = options.database.map_or_else(
        || ("default = []\ndatabase = []\n".to_owned(), String::new()),
        |database| {
            let app_feature = database.cargo_feature();
            let phoenix_features = match database {
                ProjectDatabase::All => "\"database\", \"phoenix/sqlite\", \"phoenix/pgsql\", \"phoenix/mysql\"".to_owned(),
                _ => format!("\"database\", \"phoenix/{app_feature}\""),
            };
            let toasty_features = database.toasty_features();
            (
                format!(
                    "default = [\"{app_feature}\"]\ndatabase = [\"phoenix/database\"]\n{app_feature} = [{phoenix_features}]\n"
                ),
                format!(
                    "toasty = {{ version = \"0.8\", default-features = false, features = [\"migration\", \"{toasty_features}\"] }}\n"
                ),
            )
        },
    );
    let cargo_toml = format!(
        "[package]\nname = {package}\nversion = \"0.1.0\"\nedition = \"2024\"\nrust-version = \"1.95\"\npublish = false\ndefault-run = {package}\n\n[features]\n# Database support is omitted unless selected during `px new`.\n{database_features}tls = [\"phoenix/tls\"]\nwebsocket = [\"phoenix/websocket\"]\nsse = [\"phoenix/sse\"]\nauth = [\"phoenix/auth\"]\njwt = [\"phoenix/jwt\"]\npassword = [\"phoenix/password\"]\nmetrics = [\"phoenix/metrics\"]\nredis = [\"phoenix/redis\"]\nstorage = [\"phoenix/storage\"]\nqueue = [\"phoenix/queue\"]\nmail = [\"phoenix/mail\"]\ntesting = [\"phoenix/testing\"]\n# Batch test data. Development and test only; never enable in a release build.\nfactory = [\"phoenix/factory\"]\n\n[dependencies]\n{rust_dependency}\nserde = {{ version = \"1\", features = [\"derive\"] }}\nserde_json = \"1\"\n{toasty_dependency}tokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\", \"signal\"] }}\n\n[profile.release]\ncodegen-units = 1\nlto = true\nopt-level = \"z\"\npanic = \"unwind\"\nstrip = \"symbols\"\n\n[workspace]\n",
        package = json_string(package),
        database_features = database_features,
        rust_dependency = rust_dependency,
        toasty_dependency = toasty_dependency,
    );

    let stored = StoredProjectOptions {
        frontend: options.frontend,
        database: options.database,
        render_mode: options.render_mode,
        tailwind: options.tailwind,
        features: options.features.clone(),
    };
    let extension = options.frontend.extension();

    let mut files = vec![
        ("Cargo.toml".into(), cargo_toml),
        (
            PROJECT_OPTIONS_FILE.into(),
            format_stored_options(&stored),
        ),
        ("package.json".into(), package_json),
        (".gitignore".into(), "/target\n/node_modules\n/public/assets\n/public/ssr\n/views/generated/*.ts\n/dist\n/storage/*.sqlite\n/storage/*.sqlite-*\n.env\n.DS_Store\n".to_owned()),
        (".env.example".into(), env_example_template(options.database, &options.features)),
        ("README.md".into(), project_readme(package, options)),
        ("src/main.rs".into(), main_template(&crate_name)),
        ("src/lib.rs".into(), lib_template(options.database.is_some(), &options.features)),
        ("config/mod.rs".into(), config_template(options.database.is_some())),
        ("app/controllers/mod.rs".into(), managed_modules(&["pub mod home_controller;", "pub use home_controller::HomeController;"])),
        ("app/controllers/home_controller.rs".into(), home_controller_template(options.render_mode)),
        ("app/props/mod.rs".into(), managed_modules(&[
            "pub mod demo_ssr_props;",
            "pub mod home_props;",
            "pub use demo_ssr_props::DemoSsrProps;",
            "pub use home_props::HomeProps;",
        ])),
        ("app/props/home_props.rs".into(), home_props_template()),
        ("app/props/demo_ssr_props.rs".into(), demo_ssr_props_template()),
        ("app/requests/mod.rs".into(), managed_modules(&[])),
        ("app/resources/mod.rs".into(), managed_modules(&[])),
        ("app/middleware/mod.rs".into(), managed_modules(&[])),
        ("app/commands/mod.rs".into(), commands_mod_template()),
        ("routes/web.rs".into(), home_route_template()),
        (
            format!("views/pages/home.{extension}").into(),
            home_page_template(options.frontend),
        ),
        (
            format!("views/pages/demo/spa.{extension}").into(),
            demo_spa_page_template(),
        ),
        (
            format!("views/pages/demo/ssr.{extension}").into(),
            demo_ssr_page_template(options.frontend),
        ),
        (
            format!("views/pages/demo/islands.{extension}").into(),
            demo_islands_page_template(),
        ),
        (
            format!("views/islands/counter.{extension}").into(),
            island_template("Counter", options.frontend),
        ),
        ("views/styles.css".into(), styles_template(options.tailwind)),
        ("views/generated/contracts.ts".into(), generated_contracts_template()),
        ("views/generated/routes.ts".into(), generated_routes_template()),
        ("vite.config.ts".into(), vite_template(false, options.tailwind)),
        ("vite.ssr.config.ts".into(), vite_template(true, options.tailwind)),
        ("tsconfig.json".into(), tsconfig_template()),
        ("public/.gitkeep".into(), String::new()),
        ("storage/cache/.gitkeep".into(), String::new()),
        ("storage/logs/.gitkeep".into(), String::new()),
    ];
    if options.database.is_some() {
        files.extend([
            ("app/models/mod.rs".into(), empty_model_registry()),
            (
                "database/migrations/mod.rs".into(),
                empty_migration_registry(),
            ),
            ("database/seeders/mod.rs".into(), seeder_template()),
            (
                "src/bin/phoenix-manage.rs".into(),
                management_template(
                    &crate_name,
                    options
                        .features
                        .iter()
                        .any(|feature| feature.has_migrations()),
                ),
            ),
        ]);
    }
    for feature in &options.features {
        files.push((
            format!("config/{}.toml", feature.key()).into(),
            feature_config_template(*feature),
        ));
    }
    Ok(files)
}

// One grouped template literal per section; the length is the generated file.
#[allow(clippy::too_many_lines)]
fn env_example_template(database: Option<ProjectDatabase>, features: &[ProjectFeature]) -> String {
    let database_block = match database {
        None => {
            "# ── 数据库（创建时未启用）────────────────────────────\n# 需要时用 `px new --database sqlite|pgsql|mysql` 重新创建，\n# 或按 docs/CONFIG.md 手动启用驱动 feature 后设置：\n# DATABASE_URL=sqlite:storage/app.sqlite\n".to_owned()
        }
        Some(driver) => {
            let (active, hints) = match driver {
                ProjectDatabase::Sqlite => (
                    "DATABASE_URL=sqlite:storage/app.sqlite",
                    "# DATABASE_URL=postgresql://phoenix:secret@127.0.0.1:5432/phoenix\n# DATABASE_URL=mysql://phoenix:secret@127.0.0.1:3306/phoenix\n",
                ),
                ProjectDatabase::Pgsql => (
                    "DATABASE_URL=postgresql://phoenix:secret@127.0.0.1:5432/phoenix",
                    "# DATABASE_URL=sqlite:storage/app.sqlite\n",
                ),
                ProjectDatabase::Mysql => (
                    "DATABASE_URL=mysql://phoenix:secret@127.0.0.1:3306/phoenix",
                    "# DATABASE_URL=sqlite:storage/app.sqlite\n",
                ),
                ProjectDatabase::All => (
                    "DATABASE_URL=sqlite:storage/app.sqlite",
                    "# 已编译全部驱动，改 URL 即可换库：\n# DATABASE_URL=postgresql://phoenix:secret@127.0.0.1:5432/phoenix\n# DATABASE_URL=mysql://phoenix:secret@127.0.0.1:3306/phoenix\n",
                ),
            };
            format!("# ── 数据库 ──────────────────────────────────────────\n{active}\n{hints}")
        }
    };
    let feature_block = if features.is_empty() {
        String::new()
    } else {
        let mut block =
            String::from("\n# ── Feature 密钥（config/*.toml 里以 ${VAR} 引用）─────\n");
        if features.contains(&ProjectFeature::Pay) {
            block.push_str(
                "# PAY_WECHAT_API_V3_KEY=\n# PAY_ALIPAY_APP_PRIVATE_KEY=\n# PAY_ALIPAY_PUBLIC_KEY=\n",
            );
        }
        if features.contains(&ProjectFeature::Captcha) {
            block.push_str("# 验证码无密钥；参数见 config/captcha.toml。\n");
        }
        if features.contains(&ProjectFeature::Notify) {
            block.push_str("# 通知本地用内存邮件传输；接真实 SMTP 时在此加对应密钥。\n");
        }
        block
    };
    format!(
        "# 复制为 `.env`：前后端启动 + 数据库的运行时配置都在这里。\n# 优先级：.env < 进程环境变量。config/ 目录只放 Feature 配置（TOML）。\n\n# ── 应用 ────────────────────────────────────────────\nAPP_ENV=development\nAPP_ADDR=127.0.0.1:3000\nAPP_URL=http://127.0.0.1:3000\n\n# ── 前端（Vite 开发服务器，`px dev` 使用）───────────\nVITE_DEV_URL=http://127.0.0.1:5173\n\n{database_block}\n# ── 日志 ────────────────────────────────────────────\nPHOENIX_LOG=info,hyper=warn\n\n# ── 限流与信任代理 ──────────────────────────────────\nRATE_LIMIT_REQUESTS=60\nRATE_LIMIT_WINDOW_SECONDS=60\nTRUSTED_PROXIES=none\nALLOWED_HOSTS=127.0.0.1,localhost,[::1]\n{feature_block}"
    )
}

/// `config/<feature>.toml` starter: non-secret parameters live here, secrets
/// are injected from `.env` through `${VAR}` (see `load_feature_config`).
// One template literal per Feature; the length is the generated files.
#[allow(clippy::too_many_lines)]
fn feature_config_template(feature: ProjectFeature) -> String {
    match feature {
        ProjectFeature::Captcha => {
            r"# 图形验证码 Feature（phoenix-captcha）。详见 docs/CAPTCHA.md。
#
# CaptchaConfig 目前在代码中构造（src/lib.rs 的 features()），本文件记录默认
# 可调项；修改后把对应值同步到 `CaptchaFeature::with_config(...)`。
#
# length = 5        # 每次挑战的字符数（1–16）
# width = 160       # SVG 画布宽度
# height = 60       # SVG 画布高度
# noise_curves = 3  # 干扰曲线条数
# noise_dots = 28   # 噪点数量
"
            .to_owned()
        }
        ProjectFeature::Pay => r#"# 聚合支付 Feature（phoenix-pay）。详见 docs/PAYMENTS.md。
#
# 结构留在本文件，密钥放 `.env`：字符串值支持 `${VAR}` 占位符，加载时从
# 环境注入（src/lib.rs 的 pay_manager() 通过 load_feature_config("pay") 读取）。
# 两个渠道段都注释时仅启用 MockProvider——本地/测试即可完整跑通下单与回调。

# [wechat_native]                                       # 微信支付 Native（APIv3）
# app_id = "wx1234567890"
# mch_id = "1900000001"
# mch_serial_no = ""                                    # 商户 API 证书序列号
# api_v3_key = "${PAY_WECHAT_API_V3_KEY}"               # 密钥进 .env
# private_key_path = "storage/certs/apiclient_key.pem"  # 商户私钥（不进仓库）
# notify_url = "https://example.com/pay/notify/wechat"

# [alipay_f2f]                                          # 支付宝当面付（RSA2）
# app_id = "2021000000000000"
# app_private_key = "${PAY_ALIPAY_APP_PRIVATE_KEY}"     # 密钥进 .env
# alipay_public_key = "${PAY_ALIPAY_PUBLIC_KEY}"        # 支付宝公钥（非应用公钥）
# notify_url = "https://example.com/pay/notify/alipay"
"#
        .to_owned(),
        ProjectFeature::Notify => {
            r#"# 通知 Feature（phoenix-notify）：邮件 + 数据库双通道。详见 docs/NOTIFICATIONS.md。
#
# 本地默认内存邮件传输 + 内存存储（src/lib.rs 的 notifier()）；
# 生产环境在代码中替换真实 MailTransport 与数据库 store。
#
# from = "noreply@example.com"   # 通知邮件的默认发件地址（应用代码中使用）
"#
            .to_owned()
        }
    }
}

fn config_template(database: bool) -> String {
    let database = if database {
        " (`DATABASE_URL` selects the database)"
    } else {
        ""
    };
    format!(
        "pub use phoenix::config::{{\n    AppConfig, AppConfigBuilder, ConfigError, Environment, SecretValue, load_feature_config,\n}};\n\n/// Load this application's configuration.\n///\n/// Runtime settings come from `.env`, then the process environment{database}.\n/// Feature settings live in `config/<feature>.toml` via `load_feature_config`.\n///\n/// # Errors\n///\n/// Returns a source, validation, or production-requirement error.\npub fn load() -> Result<AppConfig, ConfigError> {{\n    AppConfig::load()\n}}\n"
    )
}

// The body is one template literal; the line count is the generated file.
#[allow(clippy::too_many_lines)]
fn management_template(crate_name: &str, feature_migrations: bool) -> String {
    let migrations_expr = if feature_migrations {
        // Official Features (pay / notify) contribute migrations through their plugins.
        "{\n        let mut migrations = __PHOENIX_APP_CRATE__::migrations::all();\n        let features = __PHOENIX_APP_CRATE__::features()\n            .map_err(|error| error as Box<dyn Error>)?;\n        migrations.extend(features.into_parts().migrations);\n        migrations\n    }"
    } else {
        "__PHOENIX_APP_CRATE__::migrations::all()"
    };
    r#"#[cfg(feature = "database")]
use std::{env, error::Error, io};

#[cfg(feature = "database")]
use phoenix::database::MigrationRunner;

#[cfg(feature = "database")]
type CommandResult<T = ()> = Result<T, Box<dyn Error>>;

#[cfg(not(feature = "database"))]
fn main() {
    println!("Database support is disabled; enable the sqlite, pgsql, or mysql feature.");
}

#[cfg(feature = "database")]
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
        __PHOENIX_MIGRATIONS__,
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

#[cfg(feature = "database")]
fn require_no_options(options: &[String]) -> CommandResult {
    if options.is_empty() {
        Ok(())
    } else {
        Err(input_error(format!("unexpected arguments: {}", options.join(" "))).into())
    }
}

#[cfg(feature = "database")]
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

#[cfg(feature = "database")]
fn parse_fresh_options(options: &[String]) -> CommandResult<bool> {
    match options {
        [] => Ok(false),
        [option] if option == "--seed" => Ok(true),
        _ => Err(input_error("fresh only accepts --seed").into()),
    }
}

#[cfg(feature = "database")]
fn input_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
"#
    .replace("__PHOENIX_MIGRATIONS__", migrations_expr)
    .replace("__PHOENIX_APP_CRATE__", crate_name)
}

fn seeder_template() -> String {
    format!(
        r"use std::error::Error;

use phoenix::database::Database;

{MODULES_START}
{MODULES_END}

/// Insert repeatable development or test data.
///
/// Factories generated by `px make:model --factory` land in this directory and
/// are registered above. They compile only with the `factory` feature, so none
/// of this reaches a release build.
///
/// # Errors
///
/// Returns the first application or database error raised by a seeder.
pub async fn run(_database: &mut Database) -> Result<(), Box<dyn Error>> {{
    Ok(())
}}
"
    )
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

fn project_readme(package: &str, options: &NewProjectOptions) -> String {
    let mode = match options.render_mode {
        ProjectRenderMode::Spa => "SPA",
        ProjectRenderMode::Ssr => "SSR",
        ProjectRenderMode::Islands => "Islands",
    };
    let database = options.database.map_or_else(
        || "This project has no database dependency. Create one with `px new --database sqlite|pgsql|mysql`.".to_owned(),
        |database| format!("Selected driver: `{}` (`DATABASE_URL` in `.env`). Use `px migrate` and `px status` for migrations.", database.cargo_feature()),
    );
    let tailwind = if options.tailwind {
        "Tailwind CSS v4 is enabled through `@tailwindcss/vite`."
    } else {
        "Tailwind CSS is not enabled; add it at creation time with `px new --tailwind`."
    };
    let features = if options.features.is_empty() {
        String::new()
    } else {
        use std::fmt::Write as _;

        let mut list = String::new();
        for feature in &options.features {
            let _ = writeln!(
                list,
                "- `{key}` — {summary}（配置：`config/{key}.toml`）",
                key = feature.key(),
                summary = feature.summary(),
            );
        }
        format!(
            "\n## 已启用 Feature\n\n{list}\n装配代码在 `src/lib.rs` 的 `features()`；密钥放 `.env`，非敏感参数放 `config/*.toml`。\n\n> 注意：crates.io 上已发布的 `phoenixrs` 尚未包含 `captcha` / `pay` / `notify`\n> feature（需要 > {PHOENIXRS_VERSION} 的后续版本）。在其发布前请使用\n> `px new --framework-path <Phoenix-rs 检出>` 的本地依赖模式。\n"
        )
    };
    format!(
        "# {package}\n\nPhoenix Rust + React application.\n\n## Start\n\n```bash\ncp .env.example .env\npx dev\n```\n\n`px dev` builds the browser and renderer bundles before it starts the app, then rebuilds them whenever Rust or React source changes. The development document therefore uses the same Vite assets and renderer contract as `npm run build`.\n\n运行时配置（地址 / 数据库 / 日志 / 限流）都在 `.env`；`config/` 目录只存放 Feature 的 TOML 配置。\n\n## Rendering mode\n\nThis application starts in **{mode}** mode. Change only the page chain in `app/controllers/home_controller.rs`:\n\n```rust\n.spa()       // SPA\n.ssr()       // SSR\n.islands()   // Islands\n```\n\nThe controller, routes, renderer and build pipeline stay unchanged.\n\n首页链接了三种渲染模式的最小演示页：`/demo/spa`、`/demo/ssr`、`/demo/islands`\n（`views/pages/demo/` 与 `HomeController`，确认理解后即可删除）。\n\n## Optional integrations\n\n- {database}\n- {tailwind}\n{features}\n## Release\n\n```bash\npx release --version 0.1.0 --tarball\n```\n\n`px release:install` / `px release:rollback` 可在切换版本后执行 `deploy/restart.sh`\n（或 `--restart-cmd`）；需要自动重启时创建该脚本并 `chmod +x`。\n"
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
            let server = {crate_name}::application(config).await?.bind(&address).await?;
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

// The body is one template literal; the line count is the generated file.
#[allow(clippy::too_many_lines)]
fn lib_template(database: bool, features: &[ProjectFeature]) -> String {
    if !features.is_empty() {
        return lib_template_with_features(database, features);
    }
    r#"#[path = "../config/mod.rs"]
pub mod config;
#[path = "../app/commands/mod.rs"]
pub mod commands;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[cfg(feature = "database")]
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[cfg(feature = "database")]
#[path = "../database/migrations/mod.rs"]
pub mod migrations;
#[cfg(feature = "database")]
#[path = "../database/seeders/mod.rs"]
pub mod seeders;

use phoenix::prelude::{
    AccessLog, Application, AssetManifest, Csrf, HostAllowlist, NodeRenderer,
    NonceSecurityPolicy, RateLimit, RateLimitConfig, RendererConfig, RendererManifest, RequestId, Routes, ServeProductionAssets, SessionConfig, SessionMiddleware,
    SessionStore, StateMiddleware, TrustedProxies,
};
#[cfg(feature = "database")]
use phoenix::prelude::{Database, DatabaseError};

use config::AppConfig;

#[must_use]
#[allow(clippy::duplicate_mod)]
pub fn routes(
    config: &AppConfig,
    assets: Option<&AssetManifest>,
    renderer: &NodeRenderer,
) -> Routes {
    let session_config = SessionConfig {
        secure: config.public_url().starts_with("https://"),
        ..SessionConfig::default()
    };
    let session_store = SessionStore::memory(session_config.max_age);

    let mut routes = phoenix::mount_routes!()
        .with_middleware(TrustedProxies::new(config.trusted_proxies().iter().copied()))
        .with_middleware(RequestId)
        .with_middleware(AccessLog);
    if let Some(assets) = assets.cloned() {
        // Serve hashed Vite assets before session/CSRF so static GETs stay cheap.
        routes = routes.with_middleware(ServeProductionAssets::new(assets, "public/assets"));
    }
    routes
        .with_middleware(HostAllowlist::new(config.allowed_hosts().iter().cloned()))
        .with_middleware(RateLimit::new(RateLimitConfig {
            requests: config.rate_limit_requests(),
            window: config.rate_limit_window(),
        }))
        .with_middleware(content_security_policy(config))
        .with_middleware(SessionMiddleware::new(session_store, session_config))
        .with_middleware(Csrf)
        .with_middleware(StateMiddleware::new(config.clone()))
        .with_middleware(StateMiddleware::new(assets.cloned()))
        .with_middleware(StateMiddleware::new(renderer.clone()))
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
pub async fn application(
    config: AppConfig,
) -> Result<Application, Box<dyn std::error::Error + Send + Sync>> {
    let vite_dev_server = std::env::var_os("PHOENIX_VITE_DEV").is_some();
    let (assets, renderer) = if !vite_dev_server {
        let assets = AssetManifest::load("public/assets/phoenix-manifest.json")?;
        let renderer_manifest = RendererManifest::load("public/ssr/phoenix-renderer.json")?;
        let renderer = NodeRenderer::new(
            RendererConfig::production(&assets, &renderer_manifest, "public/ssr")?,
        );
        (Some(assets), renderer)
    } else {
        // `px dev` sets PHOENIX_VITE_DEV so this process uses Vite's browser
        // entry while HMR/full reload remains live.
        (None, NodeRenderer::new(RendererConfig::node("public/ssr/renderer.js")))
    };
    renderer.warm_up().await?;
    let built = routes(&config, assets.as_ref(), &renderer);
    // Connect the database once at startup and share it with every handler via
    // request state (`State<Database>` / `extensions().get::<Database>()`).
    #[cfg(feature = "database")]
    let built = built.with_middleware(StateMiddleware::new(database(&config).await?));
    Ok(Application::new(built)?)
}

/// Connect the configured database with every registered Toasty model.
///
/// # Errors
///
/// Returns a database error when the URL or connection is invalid.
#[cfg(feature = "database")]
pub async fn database(config: &AppConfig) -> Result<Database, DatabaseError> {
    Database::builder(models::all())
        .connect(config.database_url())
        .await
}
"#
    .to_owned()
}

/// `src/lib.rs` variant that assembles the official Features selected by
/// `px new --feature ...` (docs/FEATURES.md).
// The body is one template literal; the line count is the generated file.
#[allow(clippy::too_many_lines)]
fn lib_template_with_features(_database: bool, features: &[ProjectFeature]) -> String {
    let captcha = features.contains(&ProjectFeature::Captcha);
    let pay = features.contains(&ProjectFeature::Pay);
    let notify = features.contains(&ProjectFeature::Notify);

    let arc_import = if pay || notify {
        "\nuse std::sync::Arc;\n"
    } else {
        ""
    };

    let mut plugin_lines = String::new();
    if captcha {
        plugin_lines.push_str(
            "    // 图形验证码：注册 GET /captcha（路由名 captcha.image），依赖 Session。\n    // 自定义样式用 CaptchaFeature::with_config(...)（参数见 config/captcha.toml），\n    // 登录表单接入见 docs/CAPTCHA.md（CaptchaProtected 提取器）。\n    let features = features.plugin(phoenix::captcha::CaptchaFeature::new())?;\n",
        );
    }
    if pay {
        plugin_lines.push_str(
            "    // 聚合支付：注册回调路由（pay.notify.*）与 payments 迁移。docs/PAYMENTS.md。\n    let features = features.plugin(phoenix::pay::PayFeature::new(pay_manager()?))?;\n",
        );
    }
    if notify {
        plugin_lines.push_str(
            "    // 通知：注册 notifications 迁移；发送入口见下方 notifier()。docs/NOTIFICATIONS.md。\n    let features = features.plugin(phoenix::notify::NotifyFeature::new())?;\n",
        );
    }

    let mut helper_fns = String::new();
    if pay {
        helper_fns.push_str(
            r#"
/// 从 `config/pay.toml` 装配支付渠道；文件缺省时仅启用 Mock（本地即可闭环）。
///
/// 密钥经 `.env` 注入（`${VAR}` 占位符），非敏感参数留在 TOML。
fn pay_manager() -> Result<
    Arc<phoenix::pay::PayManager>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use phoenix::pay::{AlipayF2FProvider, MockProvider, PayManager, WechatNativeProvider};

    #[derive(Default, serde::Deserialize)]
    struct PayFileConfig {
        wechat_native: Option<phoenix::pay::WechatNativeConfig>,
        alipay_f2f: Option<phoenix::pay::AlipayF2FConfig>,
    }

    let file: PayFileConfig = phoenix::config::load_feature_config("pay")?;
    let mut builder = PayManager::builder().provider(Arc::new(MockProvider::new()));
    if let Some(wechat) = file.wechat_native {
        builder = builder.provider(Arc::new(WechatNativeProvider::new(wechat)));
    }
    if let Some(alipay) = file.alipay_f2f {
        builder = builder.provider(Arc::new(AlipayF2FProvider::new(alipay)));
    }
    Ok(Arc::new(builder.build()))
}
"#,
        );
    }
    if notify {
        helper_fns.push_str(
            r"
/// 通知发送器：本地为内存邮件传输 + 内存存储，重启即清空。
/// 上线前替换为真实 MailTransport 与数据库 store（docs/NOTIFICATIONS.md）。
#[must_use]
pub fn notifier() -> phoenix::notify::Notifier {
    let (mailer, _outbox) = phoenix::mail::Mailer::memory();
    phoenix::notify::Notifier::new()
        .with_mailer(mailer)
        .with_store(Arc::new(phoenix::notify::MemoryNotificationStore::new()))
}
",
        );
    }

    format!(
        r#"#[path = "../config/mod.rs"]
pub mod config;
#[path = "../app/commands/mod.rs"]
pub mod commands;
#[path = "../app/controllers/mod.rs"]
pub mod controllers;
#[path = "../app/middleware/mod.rs"]
pub mod middleware;
#[cfg(feature = "database")]
#[path = "../app/models/mod.rs"]
pub mod models;
#[path = "../app/props/mod.rs"]
pub mod props;
#[path = "../app/requests/mod.rs"]
pub mod requests;
#[path = "../app/resources/mod.rs"]
pub mod resources;
#[cfg(feature = "database")]
#[path = "../database/migrations/mod.rs"]
pub mod migrations;
#[cfg(feature = "database")]
#[path = "../database/seeders/mod.rs"]
pub mod seeders;
{arc_import}
use phoenix::plugin::FeatureSet;
use phoenix::prelude::{{
    AccessLog, Application, AssetManifest, Csrf, HostAllowlist, NodeRenderer,
    NonceSecurityPolicy, RateLimit, RateLimitConfig, RendererConfig, RendererManifest, RequestId, RouteGroup, Routes, ServeProductionAssets, SessionConfig, SessionMiddleware,
    SessionStore, StateMiddleware, TrustedProxies,
}};
#[cfg(feature = "database")]
use phoenix::prelude::{{Database, DatabaseError}};

use config::AppConfig;

/// `px new --feature` 选中的官方 Feature 装配（docs/FEATURES.md）。
///
/// # Errors
///
/// 返回 Feature 安装冲突（名称 / 能力 / 迁移 ID 重复）或 Feature 配置错误。
pub fn features() -> Result<FeatureSet, Box<dyn std::error::Error + Send + Sync>> {{
    let features = FeatureSet::new();
{plugin_lines}    Ok(features)
}}
{helper_fns}
/// # Errors
///
/// 返回路由冲突或 Feature 装配错误。
#[allow(clippy::duplicate_mod)]
pub fn routes(
    config: &AppConfig,
    assets: Option<&AssetManifest>,
    renderer: &NodeRenderer,
) -> Result<Routes, Box<dyn std::error::Error + Send + Sync>> {{
    let session_config = SessionConfig {{
        secure: config.public_url().starts_with("https://"),
        ..SessionConfig::default()
    }};
    let session_store = SessionStore::memory(session_config.max_age);

    let mut routes = phoenix::mount_routes!()
        // 业务路由启用 CSRF；Feature 路由不在此列——支付回调等由平台服务器
        // 调用，真实性靠验签（docs/PAYMENTS.md），挂 CSRF 会拒掉合法通知。
        .scoped(RouteGroup::new().middleware(Csrf))
        .merge(features()?.into_routes())
        .with_middleware(TrustedProxies::new(config.trusted_proxies().iter().copied()))
        .with_middleware(RequestId)
        .with_middleware(AccessLog);
    if let Some(assets) = assets.cloned() {{
        // Serve hashed Vite assets before session/CSRF so static GETs stay cheap.
        routes = routes.with_middleware(ServeProductionAssets::new(assets, "public/assets"));
    }}
    Ok(routes
        .with_middleware(HostAllowlist::new(config.allowed_hosts().iter().cloned()))
        .with_middleware(RateLimit::new(RateLimitConfig {{
            requests: config.rate_limit_requests(),
            window: config.rate_limit_window(),
        }}))
        .with_middleware(content_security_policy(config))
        // 会话全局挂载：验证码等 Feature 路由依赖 Session。
        .with_middleware(SessionMiddleware::new(session_store, session_config))
        .with_middleware(StateMiddleware::new(config.clone()))
        .with_middleware(StateMiddleware::new(assets.cloned()))
        .with_middleware(StateMiddleware::new(renderer.clone())))
}}

fn content_security_policy(config: &AppConfig) -> NonceSecurityPolicy {{
    if !config.environment().is_production() {{
        return NonceSecurityPolicy::development(
            config
                .vite_dev_url()
                .expect("development configuration always has a Vite origin"),
        )
        .expect("AppConfig validates VITE_DEV_URL as one trusted HTTP(S) origin");
    }}
    NonceSecurityPolicy::default()
}}

/// Build the Phoenix application.
///
/// # Errors
///
/// Returns a route error when route names or patterns conflict.
pub async fn application(
    config: AppConfig,
) -> Result<Application, Box<dyn std::error::Error + Send + Sync>> {{
    let vite_dev_server = std::env::var_os("PHOENIX_VITE_DEV").is_some();
    let (assets, renderer) = if !vite_dev_server {{
        let assets = AssetManifest::load("public/assets/phoenix-manifest.json")?;
        let renderer_manifest = RendererManifest::load("public/ssr/phoenix-renderer.json")?;
        let renderer = NodeRenderer::new(
            RendererConfig::production(&assets, &renderer_manifest, "public/ssr")?,
        );
        (Some(assets), renderer)
    }} else {{
        // `px dev` sets PHOENIX_VITE_DEV so this process uses Vite's browser
        // entry while HMR/full reload remains live.
        (None, NodeRenderer::new(RendererConfig::node("public/ssr/renderer.js")))
    }};
    renderer.warm_up().await?;
    let built = routes(&config, assets.as_ref(), &renderer)?;
    // Connect the database once at startup and share it with every handler via
    // request state (`State<Database>` / `extensions().get::<Database>()`).
    #[cfg(feature = "database")]
    let built = built.with_middleware(StateMiddleware::new(database(&config).await?));
    Ok(Application::new(built)?)
}}

/// Connect the configured database with every registered Toasty model.
///
/// # Errors
///
/// Returns a database error when the URL or connection is invalid.
#[cfg(feature = "database")]
pub async fn database(config: &AppConfig) -> Result<Database, DatabaseError> {{
    Database::builder(models::all())
        .connect(config.database_url())
        .await
}}
"#
    )
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

// The body is one template literal; the line count is the generated file.
#[allow(clippy::too_many_lines)]
fn home_controller_template(render_mode: ProjectRenderMode) -> String {
    format!(
        r#"use std::time::{{SystemTime, UNIX_EPOCH}};

use phoenix::prelude::{{AssetManifest, NodeRenderer, Page, Request, Response, StatusCode}};

use crate::config::AppConfig;
use crate::props::{{DemoSsrProps, HomeProps}};

pub struct HomeController;

impl HomeController {{
    /// 首页；渲染模式由 `px new` 的选择决定（当前 `{render_mode}`）。
    pub async fn index(request: Request) -> Response {{
        let page = Page::new(
            "home",
            HomeProps {{
                title: "Phoenix is ready".to_owned(),
                description: "Rust owns the application contract; React renders the page.".to_owned(),
            }},
        )
        {render_mode};
        render(&request, page).await
    }}

    /// SPA 演示：整页交给浏览器 React 渲染，任意组件可直接交互。
    pub async fn demo_spa(request: Request) -> Response {{
        render(&request, Page::new("demo/spa", serde_json::json!({{}})).spa()).await
    }}

    /// SSR 演示：服务端先渲染完整 HTML，首屏即含数据（查看网页源代码可见）。
    pub async fn demo_ssr(request: Request) -> Response {{
        let rendered_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or_else(|_| "epoch".to_owned(), |at| at.as_secs().to_string());
        render(&request, Page::new("demo/ssr", DemoSsrProps {{ rendered_at }}).ssr()).await
    }}

    /// Islands 演示：静态 HTML 中只有标记 `client:load` 的岛屿被水合。
    pub async fn demo_islands(request: Request) -> Response {{
        render(&request, Page::new("demo/islands", serde_json::json!({{}})).islands()).await
    }}
}}

/// 注入 Vite 资源并交给 Node renderer 输出（三种渲染模式共用）。
async fn render(request: &Request, mut page: Page) -> Response {{
    let renderer = request.extensions().get::<NodeRenderer>().cloned();
    let assets = request
        .extensions()
        .get::<Option<AssetManifest>>()
        .and_then(Option::as_ref);
    let vite_dev_url = request
        .extensions()
        .get::<AppConfig>()
        .and_then(AppConfig::vite_dev_url);
    if let Some(assets) = assets {{
        page = match page.production_assets(assets, "client") {{
            Ok(page) => page,
            Err(error) => {{
                return Response::text(format!("asset manifest error: {{error}}"))
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            }}
        }};
    }} else if let Some(vite_dev_url) = vite_dev_url {{
        page = page.script_src(format!(
            "{{}}/@id/__x00__virtual:phoenix/client",
            vite_dev_url.trim_end_matches('/'),
        ));
    }}
    match renderer {{
        Some(renderer) => page.respond_with_renderer(request, &renderer).await,
        None => Response::text("Phoenix renderer is unavailable")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR),
    }}
}}
"#,
        render_mode = render_mode.page_builder(),
    )
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

fn demo_ssr_props_template() -> String {
    r#"use serde::Serialize;

/// SSR 演示页数据：由服务端注入，首屏 HTML 即包含（`views/pages/demo/ssr`）。
#[phoenix::contract(page, page = "demo/ssr")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoSsrProps {
    pub rendered_at: String,
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
        // 三种渲染模式的最小演示页（确认理解后可整组删除）。
        .get("/demo/spa", HomeController::demo_spa)
        .name("demo.spa")
        .get("/demo/ssr", HomeController::demo_ssr)
        .name("demo.ssr")
        .get("/demo/islands", HomeController::demo_islands)
        .name("demo.islands")
}
"#
    .to_owned()
}

const HOME_PAGE_BODY: &str = r#"  return (
    <main className="welcome">
      <p className="eyebrow">PHOENIX / RUST + REACT</p>
      <h1>{title}</h1>
      <p>{description}</p>
      {/* 同一套控制器 / 路由下的三种渲染模式，每页一个最小演示。 */}
      <ul className="demo-links">
        <li>
          <Link href={demo.spa()}>SPA</Link> — 整页浏览器渲染，交互最自由
        </li>
        <li>
          <Link href={demo.ssr()}>SSR</Link> — 服务端渲染完整 HTML，首屏即含数据
        </li>
        <li>
          <Link href={demo.islands()}>Islands</Link> — 静态页里只水合少量交互岛
        </li>
      </ul>
      <code>px make:model Post --all</code>
    </main>
  );
}
"#;

fn home_page_template(frontend: ProjectFrontend) -> String {
    let (imports, signature) = match frontend {
        ProjectFrontend::Tsx => (
            concat!(
                "import { Link } from \"@apizero/react\";\n",
                "import type { HomeProps } from \"../generated/contracts.js\";\n",
                "import { demo } from \"../generated/routes.js\";",
            ),
            "export default function Home({ title, description }: HomeProps) {",
        ),
        ProjectFrontend::Jsx => (
            concat!(
                "import { Link } from \"@apizero/react\";\n",
                "import { demo } from \"../generated/routes.js\";",
            ),
            "export default function Home({ title, description }) {",
        ),
    };
    format!("{imports}\n\n{signature}\n{HOME_PAGE_BODY}")
}

/// SPA 演示页：整页 hydrate，`useState` 计数器即时生效（TSX/JSX 同文）。
fn demo_spa_page_template() -> String {
    r#"import { useState } from "react";
import { Link } from "@apizero/react";
import { home } from "../../generated/routes.js";

// SPA：整页都在浏览器渲染——查看网页源代码只有挂载点，没有正文。
export default function DemoSpa() {
  const [count, setCount] = useState(0);
  return (
    <main className="welcome">
      <p className="eyebrow">DEMO / SPA</p>
      <h1>SPA 模式</h1>
      <p>本页任何组件都可以直接使用浏览器状态与事件。</p>
      <button type="button" onClick={() => setCount((value) => value + 1)}>
        点击了 {count} 次
      </button>
      <p>
        <Link href={home()}>← 返回首页</Link>
      </p>
    </main>
  );
}
"#
    .to_owned()
}

fn demo_ssr_page_template(frontend: ProjectFrontend) -> String {
    let (imports, signature) = match frontend {
        ProjectFrontend::Tsx => (
            concat!(
                "import { Link } from \"@apizero/react\";\n",
                "import type { DemoSsrProps } from \"../../generated/contracts.js\";\n",
                "import { home } from \"../../generated/routes.js\";",
            ),
            "export default function DemoSsr({ renderedAt }: DemoSsrProps) {",
        ),
        ProjectFrontend::Jsx => (
            concat!(
                "import { Link } from \"@apizero/react\";\n",
                "import { home } from \"../../generated/routes.js\";",
            ),
            "export default function DemoSsr({ renderedAt }) {",
        ),
    };
    // SSR：完整 HTML 在服务端生成后再 hydrate。
    format!(
        r#"{imports}

// SSR：服务端渲染完整 HTML——查看网页源代码能直接看到下面的数据。
{signature}
  return (
    <main className="welcome">
      <p className="eyebrow">DEMO / SSR</p>
      <h1>SSR 模式</h1>
      <p>本页由服务端渲染，Rust 注入的时间戳：{{renderedAt}}。</p>
      <p>刷新后时间戳变化；右键「查看网页源代码」可在 HTML 里找到它。</p>
      <p>
        <Link href={{home()}}>← 返回首页</Link>
      </p>
    </main>
  );
}}
"#
    )
}

/// Islands 演示页：静态正文 + 一个 `client:load` 岛（TSX/JSX 同文）。
fn demo_islands_page_template() -> String {
    r#"import { Link } from "@apizero/react";
import Counter from "../../islands/counter";
import { home } from "../../generated/routes.js";

// Islands：页面主体是静态 HTML，只有标记 client:load 的组件会加载浏览器代码。
export default function DemoIslands() {
  return (
    <main className="welcome">
      <p className="eyebrow">DEMO / ISLANDS</p>
      <h1>Islands 模式</h1>
      <p>下面的计数器是本页唯一被水合的交互岛，其余内容零 JS。</p>
      <Counter client:load />
      <p>
        <Link href={home()}>← 返回首页</Link>
      </p>
    </main>
  );
}
"#
    .to_owned()
}

fn styles_template(tailwind: bool) -> String {
    let tailwind = if tailwind {
        "@import \"tailwindcss\";\n\n"
    } else {
        ""
    };
    format!(
        r"{tailwind}:root {{
  font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  color: #172033;
  background: #f5f7fb;
}}
* {{ box-sizing: border-box; }}
body {{ margin: 0; min-width: 320px; min-height: 100vh; }}
.welcome {{ width: min(760px, calc(100% - 40px)); margin: 16vh auto 0; }}
.eyebrow {{ color: #315bd6; font-size: 12px; font-weight: 800; letter-spacing: 0.14em; }}
h1 {{ margin: 12px 0; font-size: clamp(42px, 8vw, 76px); line-height: 0.98; }}
.welcome > p:not(.eyebrow) {{ max-width: 640px; color: #5d6879; font-size: 18px; line-height: 1.7; }}
code {{ display: inline-block; margin-top: 18px; padding: 12px 14px; border: 1px solid #d7dce5; background: white; }}
a {{ color: #315bd6; }}
button {{ margin-top: 8px; padding: 10px 16px; border: 1px solid #d7dce5; background: white; font-size: 16px; cursor: pointer; }}
.demo-links {{ margin: 18px 0 0; padding-left: 20px; }}
.demo-links li {{ margin: 6px 0; color: #5d6879; }}
"
    )
}

fn generated_contracts_template() -> String {
    r#"// Generated by Phoenix. Vite will refresh this file from Rust contracts.
export interface HomeProps {
  title: string;
  description: string;
}
export interface DemoSsrProps {
  renderedAt: string;
}
export interface PhoenixPageProps {
  home: HomeProps;
  "demo/ssr": DemoSsrProps;
}
export type PhoenixSharedProps = Record<string, never>;
export const contractHash = "scaffold" as const;
"#
    .to_owned()
}

fn generated_routes_template() -> String {
    r#"// Generated by Phoenix. Vite will refresh this file from Rust routes.
export const routes = {
  home: "home",
  demo: { spa: "demo.spa", ssr: "demo.ssr", islands: "demo.islands" },
} as const;
export type PhoenixRouteName = "home" | "demo.spa" | "demo.ssr" | "demo.islands";
export const home = routes.home;
export const demo = routes.demo;
"#
    .to_owned()
}

fn vite_template(renderer: bool, tailwind: bool) -> String {
    let tailwind_import = if tailwind {
        "import tailwindcss from \"@tailwindcss/vite\";\n"
    } else {
        ""
    };
    let tailwind_plugin = if tailwind { ", tailwindcss()" } else { "" };
    format!(
        "import {{ defineConfig }} from \"vite\";\nimport {{ phoenix }} from \"@apizero/vite\";\n{tailwind_import}\nexport default defineConfig({{\n  plugins: [phoenix({renderer}){tailwind_plugin}],\n}});\n",
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
  "include": ["views/**/*.js", "views/**/*.jsx", "views/**/*.ts", "views/**/*.tsx", "vite.config.ts", "vite.ssr.config.ts"]
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

/// A factory for `model`, wired to whichever parent it belongs to.
///
/// Development and test only: the file is compiled behind the `factory`
/// feature, so it never reaches a release build.
fn factory_template(model: &QualifiedName, relations: &[Relation]) -> String {
    let class = &model.class;
    let parent = relations
        .iter()
        .find(|relation| relation.kind == RelationKind::BelongsTo);

    let (signature, extra) = parent.map_or_else(
        || (String::from("|f|"), String::new()),
        |relation| {
            let leaf = relation
                .target
                .rsplit("::")
                .next()
                .unwrap_or(&relation.target);
            let key = format!("{}_id", snake_case(leaf));
            (
                format!("|f, {key}: i64|"),
                format!("\n        .{key}({key})"),
            )
        },
    );
    let usage = parent.map_or_else(
        || format!("seeder.create::<{class}>(10).await?"),
        |relation| {
            let leaf = relation
                .target
                .rsplit("::")
                .next()
                .unwrap_or(&relation.target);
            format!(
                "seeder.create_with::<{class}, _>(5, {}.id).await?",
                snake_case(leaf)
            )
        },
    );

    format!(
        r#"//! Generated rows for `{class}`. **Development and test only** — the
//! whole file disappears without the `factory` feature.
//!
//! Seed with: `{usage}`
#![cfg(feature = "factory")]

use crate::models::{class};

phoenix::factory! {{
    {class}, {signature} {class}::create()
        .name(f.name()){extra},
}}
"#
    )
}

/// Write a factory and register its module.
///
/// Not `add_rust_item`: that also re-exports a type named after the file, and a
/// factory file declares no type — it only implements a trait for the model.
fn add_factory(
    editor: &mut ProjectEditor,
    model: &QualifiedName,
    relations: &[Relation],
) -> Result<(), ScaffoldError> {
    let module = format!("{}_factory", snake_case(&model.class));
    editor.create(
        PathBuf::from("database/seeders").join(format!("{module}.rs")),
        factory_template(model, relations),
    )?;
    editor.update_managed_lines(
        Path::new("database/seeders/mod.rs"),
        MODULES_START,
        MODULES_END,
        &[format!("pub mod {module};")],
    )
}

fn add_model(
    editor: &mut ProjectEditor,
    model: &QualifiedName,
    relations: &[Relation],
) -> Result<(), ScaffoldError> {
    add_rust_item(
        editor,
        "app/models",
        model,
        &model_template(&model.class, relations),
    )?;
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

/// Register the `users` table migration generated by `px make:auth`.
///
/// Idempotent: a `--force` rebuild reuses the existing migration rather than
/// stacking a second, duplicate `create_users_table` file.
fn add_auth_user_migration(editor: &mut ProjectEditor) -> Result<(), ScaffoldError> {
    if editor
        .read(Path::new("database/migrations/mod.rs"))?
        .contains("create_users_table")
    {
        return Ok(());
    }
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ScaffoldError::InvalidClock)?
        .as_millis();
    let id = milliseconds.to_string();
    let module = format!("m_{id}_create_users_table");
    editor.create(
        format!("database/migrations/{module}.rs"),
        auth_user_migration_template(&id),
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

/// Ensure every named feature is present in the project's `[features] default`
/// array so a bare `cargo check` compiles the generated slice.
fn enable_project_features(
    editor: &mut ProjectEditor,
    features: &[&str],
) -> Result<(), ScaffoldError> {
    let relative = PathBuf::from("Cargo.toml");
    let manifest = editor.read(&relative)?;
    if manifest.is_empty() {
        return Err(ScaffoldError::InvalidManagedFile(
            editor.root.join(&relative),
        ));
    }
    editor.set(relative, ensure_default_features(&manifest, features))
}

/// Add `features` to the `default = [...]` array in a Cargo manifest, keeping
/// existing entries and order, and leaving the rest of the file untouched.
fn ensure_default_features(manifest: &str, features: &[&str]) -> String {
    let mut updated = manifest
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let is_default = trimmed
                .strip_prefix("default")
                .is_some_and(|rest| rest.trim_start().starts_with('='));
            if is_default
                && let Some(open) = line.find('[')
                && let Some(close_offset) = line[open..].find(']')
            {
                let close = open + close_offset;
                let mut entries = line[open + 1..close]
                    .split(',')
                    .map(|entry| entry.trim().trim_matches('"').to_owned())
                    .filter(|entry| !entry.is_empty())
                    .collect::<Vec<_>>();
                for feature in features {
                    if !entries.iter().any(|entry| entry == feature) {
                        entries.push((*feature).to_owned());
                    }
                }
                let indent = &line[..line.len() - trimmed.len()];
                let rendered = entries
                    .iter()
                    .map(|entry| format!("\"{entry}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!("{indent}default = [{rendered}]");
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    if manifest.ends_with('\n') {
        updated.push('\n');
    }
    updated
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

fn add_page(
    editor: &mut ProjectEditor,
    page: &PageName,
    frontend: ProjectFrontend,
) -> Result<(), ScaffoldError> {
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
        "{}.{}",
        kebab_case(page.parts.last().expect("page has one part")),
        frontend.extension()
    ));
    editor.create(
        path,
        page_template(&page.class, &props.class, page.parts.len(), frontend),
    )
}

fn page_props_name(page: &PageName) -> QualifiedName {
    QualifiedName {
        modules: page.parts[..page.parts.len() - 1].to_vec(),
        class: format!("{}Props", page.class),
    }
}

/// Register the Rust page-props contract and write one auth page component.
fn add_auth_page(
    editor: &mut ProjectEditor,
    name: &str,
    content: String,
    frontend: ProjectFrontend,
) -> Result<(), ScaffoldError> {
    let page = PageName::parse(name)?;
    let props = page_props_name(&page);
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
        "{}.{}",
        kebab_case(page.parts.last().expect("page has one part")),
        frontend.extension()
    ));
    editor.create(path, content)
}

/// A model written the short way: conventions filled in by `#[phoenix::model]`,
/// relations expressed as one line each.
fn model_template(name: &str, relations: &[Relation]) -> String {
    use std::fmt::Write as _;

    let mut imports = String::from("use phoenix::model;\n");
    if !relations.is_empty() {
        imports.push_str("use phoenix::database::Deferred;\n\n");
        // Related models are reachable through the registry's re-exports, so a
        // relation never needs the target's module path spelled out.
        let mut targets: Vec<&str> = relations
            .iter()
            .map(|relation| {
                let target = relation.target.as_str();
                target.rsplit("::").next().unwrap_or(target)
            })
            .collect();
        targets.sort_unstable();
        targets.dedup();
        let _ = writeln!(imports, "use crate::models::{{{}}};", targets.join(", "));
    }

    let mut fields = String::from("    pub name: String,\n");
    for relation in relations {
        let attribute = relation.kind.attribute();
        let target = &relation.target;
        let leaf = target.rsplit("::").next().unwrap_or(target);
        let field = snake_case(leaf);
        let (field, ty) = match relation.kind {
            // `#[belongs_to] user: Deferred<User>` — the foreign key and the
            // column it maps to are conventions, so neither is written here.
            RelationKind::BelongsTo | RelationKind::HasOne => {
                (field, format!("Deferred<{target}>"))
            }
            RelationKind::HasMany => (pluralize(&field), format!("Deferred<Vec<{target}>>")),
        };
        let _ = write!(fields, "    #[{attribute}]\n    pub {field}: {ty},\n");
    }

    format!(
        "{imports}
/// `#[phoenix::model]` fills in the table name, an auto-increment `id`, the
/// derives, and every relation's foreign key. Write any of them yourself to
/// take that part back.
#[model]
pub struct {name} {{
{fields}}}
"
    )
}

/// The persistent `User` model backing `px make:auth`.
fn auth_user_model_template() -> String {
    r"use phoenix::database::Model;

/// Registered account. Passwords are stored only as Argon2id PHC hashes
/// (`password_hash`); the plaintext never touches the database.
#[derive(Clone, Debug, Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: u64,
    #[unique]
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: String,
}
"
    .to_owned()
}

/// The `users` table migration (portable across sqlite / pgsql / mysql).
fn auth_user_migration_template(id: &str) -> String {
    format!(
        r#"use phoenix::database::Migration;

#[must_use]
pub fn migration() -> Migration {{
    Migration::new("{id}", "create users table")
        .up(
            "CREATE TABLE users (\
             id BIGINT PRIMARY KEY, \
             email VARCHAR(255) NOT NULL UNIQUE, \
             name VARCHAR(255) NOT NULL, \
             password_hash VARCHAR(255) NOT NULL, \
             created_at VARCHAR(64) NOT NULL)",
        )
        .down("DROP TABLE users")
}}
"#
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

fn auth_routes_template() -> String {
    r#"//! 认证路由：由 `px make:auth` 生成，`mount_routes!()` 自动收集本文件。
//!
//! 图形验证码（可选，默认以注释交付）：`CaptchaFeature` 位于独立 crate
//! `phoenix-captcha`，未随 `phoenix` 门面导出，因此**不是零配置能力**，
//! 脚手架不默认开启。启用步骤（详见 docs/CAPTCHA.md）：
//! 1. 应用 `Cargo.toml` 添加依赖 `phoenix-captcha`；
//! 2. `FeatureSet::new().plugin(CaptchaFeature::new())?` 注册 `GET /captcha`
//!    （命名路由 `captcha.image`），并把 Feature 路由合并进应用路由；
//! 3. `LoginInput` 增加 `captcha` 字段并实现 `CaptchaInput`，登录 handler
//!    换用 `CaptchaProtected` 提取器；
//! 4. 解开 `views/pages/auth/login` 页面里的 `CaptchaImage` 注释块。

use std::time::Duration;

use phoenix::prelude::{RateLimit, RateLimitConfig, RouteGroup, Routes, typed};

use crate::controllers::AuthController;
use crate::requests::{LoginInput, PasswordResetInput, RegisterInput};
use crate::resources::{AuthMessageResource, AuthSessionResource};

#[must_use]
pub fn routes() -> Routes {
    // 认证提交接口在全局限流之上叠加更严格的默认（每客户端 IP 每秒 2 次），
    // 抑制凭据爆破与重置邮件轰炸；次数与窗口按业务调整。
    let auth_rate_limit = RateLimit::new(RateLimitConfig {
        requests: 2,
        window: Duration::from_secs(1),
    });

    Routes::new()
        .get("/login", AuthController::show_login)
        .name("login.show")
        .get("/register", AuthController::show_register)
        .name("register.show")
        .get("/forgot-password", AuthController::show_forgot_password)
        .name("password-reset.show")
        .group(RouteGroup::new().middleware(auth_rate_limit), |routes| {
            routes
                .post("/login", AuthController::login)
                .name("login.store")
                .action::<LoginInput, AuthSessionResource>()
                .post("/logout", AuthController::logout)
                .name("logout.store")
                .post("/register", AuthController::register)
                .name("register.store")
                .action::<RegisterInput, AuthSessionResource>()
                .post(
                    "/password-reset",
                    typed(AuthController::request_password_reset),
                )
                .name("password-reset.store")
                .action::<PasswordResetInput, AuthMessageResource>()
        })
}
"#
    .to_owned()
}

// The body is one template literal; the line count is the generated file, not logic.
#[allow(clippy::too_many_lines)]
fn auth_controller_template() -> String {
    r#"use phoenix::auth::{AuthError, AuthGuard, AuthUser, UserProvider};
use phoenix::database::create;
use phoenix::prelude::{
    AssetManifest, BoxFuture, Database, FromRequest, IntoResponse, Json, Page, PageResponseError,
    Password, Request, Response, Session, StatusCode, Validated,
};
use serde_json::json;

use crate::config::AppConfig;
use crate::models::User;
use crate::props::auth::{AuthForgotPasswordProps, AuthLoginProps, AuthRegisterProps};
use crate::requests::{LoginInput, PasswordResetInput, RegisterInput};
use crate::resources::{AuthMessageResource, AuthSessionResource};

/// `UserProvider` backed by the persistent `users` table.
///
/// Holds a cloned `Database` handle (a shared connection pool), so each lookup
/// runs on its own executor without borrowing the request.
struct DatabaseUserProvider {
    database: Database,
}

impl DatabaseUserProvider {
    fn new(database: Database) -> Self {
        Self { database }
    }
}

impl UserProvider for DatabaseUserProvider {
    fn find_by_identifier(
        &self,
        identifier: &str,
    ) -> BoxFuture<Result<Option<AuthUser>, AuthError>> {
        let mut executor = self.database.toasty().clone();
        let email = identifier.trim().to_ascii_lowercase();
        Box::pin(async move {
            match User::filter_by_email(email).get(&mut executor).await {
                Ok(user) => Ok(Some(auth_user(&user))),
                Err(error) if error.is_record_not_found() => Ok(None),
                Err(error) => Err(AuthError::provider(error.to_string())),
            }
        })
    }

    fn find_by_id(&self, id: &str) -> BoxFuture<Result<Option<AuthUser>, AuthError>> {
        let mut executor = self.database.toasty().clone();
        let parsed = id.parse::<u64>();
        Box::pin(async move {
            let Ok(id) = parsed else { return Ok(None) };
            match User::filter_by_id(id).get(&mut executor).await {
                Ok(user) => Ok(Some(auth_user(&user))),
                Err(error) if error.is_record_not_found() => Ok(None),
                Err(error) => Err(AuthError::provider(error.to_string())),
            }
        })
    }
}

/// Map a persisted row onto the framework's minimal `AuthUser` (password hash
/// included so the guard can verify it with Argon2id).
fn auth_user(user: &User) -> AuthUser {
    AuthUser::new(
        user.id.to_string(),
        user.email.clone(),
        user.password_hash.clone(),
    )
    .with_name(user.name.clone())
    .with_role("member")
}

pub struct AuthController;

impl AuthController {
    #[allow(clippy::unused_async)]
    pub async fn show_login(request: Request) -> Response {
        render_auth_page(
            &request,
            Page::new(
                "auth/login",
                AuthLoginProps {
                    title: "登录".to_owned(),
                },
            ),
        )
    }

    #[allow(clippy::unused_async)]
    pub async fn show_register(request: Request) -> Response {
        render_auth_page(
            &request,
            Page::new(
                "auth/register",
                AuthRegisterProps {
                    title: "注册".to_owned(),
                },
            ),
        )
    }

    #[allow(clippy::unused_async)]
    pub async fn show_forgot_password(request: Request) -> Response {
        render_auth_page(
            &request,
            Page::new(
                "auth/forgot-password",
                AuthForgotPasswordProps {
                    title: "找回密码".to_owned(),
                },
            ),
        )
    }

    /// 邮箱 + 密码登录，成功后开启会话。
    ///
    /// 校验交给 `phoenix::auth::AuthGuard`：内部用 Argon2id 比对 `password_hash`，
    /// 成功后轮换会话 id（OWASP 会话固定防护）并写入 subject。
    /// 可选图形验证码：启用 `phoenix-captcha` 后改用
    /// `CaptchaProtected<Validated<Json<LoginInput>>>` 提取器（docs/CAPTCHA.md）。
    pub async fn login(request: Request) -> Response {
        let Validated(Json(input)) = match Validated::<Json<LoginInput>>::from_request(&request) {
            Ok(input) => input,
            Err(rejection) => return rejection.into_response(),
        };
        let Some(session) = request.extensions().get::<Session>().cloned() else {
            return session_unavailable();
        };
        let Some(database) = request.extensions().get::<Database>().cloned() else {
            return database_unavailable();
        };
        let provider = DatabaseUserProvider::new(database);
        let guard = AuthGuard::new(&provider, &session);
        match guard.attempt(&input.email, &input.password).await {
            Ok(Some(user)) => Json(AuthSessionResource {
                subject: user.identifier().to_owned(),
                name: user.name().to_owned(),
                role: "member".to_owned(),
            })
            .into_response(),
            Ok(None) => (
                StatusCode::UNAUTHORIZED,
                Json(AuthMessageResource {
                    message: "邮箱或密码不正确。".to_owned(),
                }),
            )
                .into_response(),
            Err(error) => provider_error(&error),
        }
    }

    /// 注册新用户：唯一邮箱查重、Argon2id 哈希入库，随后自动登录。
    pub async fn register(request: Request) -> Response {
        let Validated(Json(input)) = match Validated::<Json<RegisterInput>>::from_request(&request)
        {
            Ok(input) => input,
            Err(rejection) => return rejection.into_response(),
        };
        let Some(session) = request.extensions().get::<Session>().cloned() else {
            return session_unavailable();
        };
        let Some(mut database) = request.extensions().get::<Database>().cloned() else {
            return database_unavailable();
        };
        let email = input.email.trim().to_ascii_lowercase();
        let provider = DatabaseUserProvider::new(database.clone());
        match provider.find_by_identifier(&email).await {
            Ok(Some(_)) => return email_taken(),
            Ok(None) => {}
            Err(error) => return provider_error(&error),
        }
        let password_hash = match Password::hash(&input.password) {
            Ok(hash) => hash,
            Err(error) => {
                return Response::text(format!("password hashing failed: {error}"))
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs().to_string())
            .unwrap_or_default();
        let created = create!(User {
            email: email.clone(),
            name: input.name.clone(),
            password_hash: password_hash,
            created_at: created_at,
        })
        .exec(database.toasty_mut())
        .await;
        match created {
            Ok(user) => {
                // AuthGuard::login rotates the session id and stores the subject.
                AuthGuard::new(&provider, &session).login(&auth_user(&user));
                (
                    StatusCode::CREATED,
                    Json(AuthSessionResource {
                        subject: user.email,
                        name: user.name,
                        role: "member".to_owned(),
                    }),
                )
                    .into_response()
            }
            Err(error) => Response::text(format!("failed to create user: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    /// 退出登录并销毁会话。
    #[allow(clippy::unused_async)]
    pub async fn logout(request: Request) -> Json<AuthMessageResource> {
        if let Some(session) = request.extensions().get::<Session>() {
            session.destroy();
        }
        Json(AuthMessageResource {
            message: "已退出登录。".to_owned(),
        })
    }

    /// 密码重置请求：为防账号枚举，无论邮箱是否存在都返回同一响应。
    #[allow(clippy::unused_async)]
    pub async fn request_password_reset(
        Validated(Json(_input)): Validated<Json<PasswordResetInput>>,
    ) -> (StatusCode, Json<AuthMessageResource>) {
        // TODO: 生成重置 token 并通过 phoenix-mail 发送（脚手架不落地邮件）。
        (
            StatusCode::ACCEPTED,
            Json(AuthMessageResource {
                message: "如果该邮箱已注册，稍后会收到重置说明。".to_owned(),
            }),
        )
    }
}

/// 邮箱已注册：返回 422 校验错误信封，字段错误对齐 `Validated` 的响应形状。
fn email_taken() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "message": "The submitted data is invalid.",
            "errors": {
                "email": [{ "rule": "unique", "message": "该邮箱已注册。" }]
            }
        })),
    )
        .into_response()
}

fn session_unavailable() -> Response {
    Response::text("SessionMiddleware is not mounted")
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

fn database_unavailable() -> Response {
    // application() connects the database and mounts it via StateMiddleware.
    Response::text("database is unavailable")
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

fn provider_error(error: &AuthError) -> Response {
    Response::text(format!("auth store error: {error}"))
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
}

/// 认证页以 SPA 协议渲染（表单需要可交互），并把 CSRF token 注入页面信封，
/// 生成的 typed action 会自动携带 `X-CSRF-Token`。
fn render_auth_page(request: &Request, page: Page) -> Response {
    let mut page = page.spa();
    if let Some(token) = request
        .extensions()
        .get::<Session>()
        .and_then(Session::csrf_token)
    {
        page = page.csrf_token(token);
    }
    let assets = request
        .extensions()
        .get::<Option<AssetManifest>>()
        .and_then(Option::as_ref);
    if let Some(assets) = assets {
        page = match page.production_assets(assets, "client") {
            Ok(page) => page,
            Err(error) => {
                return Response::text(format!("asset manifest error: {error}"))
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
    } else if let Some(vite_dev_url) = request
        .extensions()
        .get::<AppConfig>()
        .and_then(AppConfig::vite_dev_url)
    {
        page = page.script_src(format!(
            "{}/@id/__x00__virtual:phoenix/client",
            vite_dev_url.trim_end_matches('/'),
        ));
    }
    page.respond_to(request, None)
        .unwrap_or_else(PageResponseError::into_response)
}
"#
    .to_owned()
}

fn login_input_template() -> String {
    r#"use phoenix::prelude::{
    Validate, ValidationErrors, Validator, max_length, min_length, required, rules, string,
};
use serde::Deserialize;

/// 登录表单入参。
///
/// 启用图形验证码后：增加 `pub captcha: String` 字段并实现
/// `phoenix_captcha::CaptchaInput`（见 routes/auth.rs 顶部说明）。
#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub email: String,
    pub password: String,
}

impl Validate for LoginInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({ "email": self.email, "password": self.password });
        Validator::new(&data)
            .field(
                "email",
                rules![required(), string(), min_length(3), max_length(120)],
            )
            .field(
                "password",
                rules![required(), string(), min_length(8), max_length(1024)],
            )
            .validate()
    }
}
"#
    .to_owned()
}

fn register_input_template() -> String {
    r#"use phoenix::prelude::{
    Validate, ValidationErrors, Validator, max_length, min_length, required, rules, string,
};
use serde::Deserialize;

/// 注册表单入参。
#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct RegisterInput {
    pub name: String,
    pub email: String,
    pub password: String,
}

impl Validate for RegisterInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({
            "name": self.name,
            "email": self.email,
            "password": self.password,
        });
        Validator::new(&data)
            .field(
                "name",
                rules![required(), string(), min_length(1), max_length(40)],
            )
            .field(
                "email",
                rules![required(), string(), min_length(3), max_length(120)],
            )
            .field(
                "password",
                rules![required(), string(), min_length(8), max_length(1024)],
            )
            .validate()
    }
}
"#
    .to_owned()
}

fn password_reset_input_template() -> String {
    r#"use phoenix::prelude::{
    Validate, ValidationErrors, Validator, max_length, min_length, required, rules, string,
};
use serde::Deserialize;

/// 密码重置请求入参。
#[phoenix::contract(input)]
#[derive(Debug, Deserialize)]
pub struct PasswordResetInput {
    pub email: String,
}

impl Validate for PasswordResetInput {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let data = serde_json::json!({ "email": self.email });
        Validator::new(&data)
            .field(
                "email",
                rules![required(), string(), min_length(3), max_length(120)],
            )
            .validate()
    }
}
"#
    .to_owned()
}

fn auth_session_resource_template() -> String {
    r#"use serde::Serialize;

/// 登录 / 注册成功后返回给浏览器的会话信息。
#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionResource {
    pub subject: String,
    pub name: String,
    pub role: String,
}
"#
    .to_owned()
}

fn auth_message_resource_template() -> String {
    r#"use serde::Serialize;

/// 认证流程的通用提示消息。
#[phoenix::contract(resource)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMessageResource {
    pub message: String,
}
"#
    .to_owned()
}

const AUTH_LOGIN_PAGE_BODY: &str = r#"  return (
    <main className="auth-page">
      <h1>{title}</h1>
      <Form
        action={login.store}
        initialValues={{ email: "", password: "" }}
        fields={LoginInputFields}
        onSuccess={() => redirect(home())}
      >
        {(form) => (
          <>
            <label htmlFor="login-email">邮箱</label>
            <input id="login-email" type="email" autoComplete="email" {...form.field("email")} />
            <FieldError errors={form.errors} name="email" />

            <label htmlFor="login-password">密码</label>
            <input
              id="login-password"
              type="password"
              autoComplete="current-password"
              {...form.field("password")}
            />
            <FieldError errors={form.errors} name="password" />

            {/* 启用图形验证码后解开（Rust 侧装配见 routes/auth.rs 顶部注释与 docs/CAPTCHA.md）：
            <CaptchaImage />
            <input autoComplete="off" {...form.field("captcha")} />
            <FieldError errors={form.errors} name="captcha" />
            */}

            <button type="submit" disabled={form.processing}>
              {form.processing ? "登录中…" : "登录"}
            </button>
          </>
        )}
      </Form>
      <p>
        还没有账号？<Link href={register.show()}>注册</Link>
        <span> · </span>
        <Link href={routes["password-reset"].show()}>忘记密码？</Link>
      </p>
    </main>
  );
}
"#;

fn auth_login_page_template(frontend: ProjectFrontend) -> String {
    let (imports, signature) = match frontend {
        ProjectFrontend::Tsx => (
            concat!(
                "import { FieldError, Form, Link, redirect } from \"@apizero/react\";\n",
                "// 启用图形验证码后解开：\n",
                "// import { CaptchaImage } from \"@apizero/react\";\n",
                "import type { AuthLoginProps } from \"../../generated/contracts.js\";\n",
                "import { LoginInputFields } from \"../../generated/contracts.js\";\n",
                "import { home, login, register, routes } from \"../../generated/routes.js\";",
            ),
            "export default function AuthLogin({ title }: AuthLoginProps) {",
        ),
        ProjectFrontend::Jsx => (
            concat!(
                "import { FieldError, Form, Link, redirect } from \"@apizero/react\";\n",
                "// 启用图形验证码后解开：\n",
                "// import { CaptchaImage } from \"@apizero/react\";\n",
                "import { LoginInputFields } from \"../../generated/contracts.js\";\n",
                "import { home, login, register, routes } from \"../../generated/routes.js\";",
            ),
            "export default function AuthLogin({ title }) {",
        ),
    };
    format!("{imports}\n\n{signature}\n{AUTH_LOGIN_PAGE_BODY}")
}

const AUTH_REGISTER_PAGE_BODY: &str = r#"  return (
    <main className="auth-page">
      <h1>{title}</h1>
      <Form
        action={register.store}
        initialValues={{ name: "", email: "", password: "" }}
        fields={RegisterInputFields}
        onSuccess={() => redirect(home())}
      >
        {(form) => (
          <>
            <label htmlFor="register-name">昵称</label>
            <input id="register-name" autoComplete="nickname" {...form.field("name")} />
            <FieldError errors={form.errors} name="name" />

            <label htmlFor="register-email">邮箱</label>
            <input
              id="register-email"
              type="email"
              autoComplete="email"
              {...form.field("email")}
            />
            <FieldError errors={form.errors} name="email" />

            <label htmlFor="register-password">密码</label>
            <input
              id="register-password"
              type="password"
              autoComplete="new-password"
              {...form.field("password")}
            />
            <FieldError errors={form.errors} name="password" />

            <button type="submit" disabled={form.processing}>
              {form.processing ? "注册中…" : "注册"}
            </button>
          </>
        )}
      </Form>
      <p>
        已有账号？<Link href={login.show()}>登录</Link>
      </p>
    </main>
  );
}
"#;

fn auth_register_page_template(frontend: ProjectFrontend) -> String {
    let (imports, signature) = match frontend {
        ProjectFrontend::Tsx => (
            concat!(
                "import { FieldError, Form, Link, redirect } from \"@apizero/react\";\n",
                "import type { AuthRegisterProps } from \"../../generated/contracts.js\";\n",
                "import { RegisterInputFields } from \"../../generated/contracts.js\";\n",
                "import { home, login, register } from \"../../generated/routes.js\";",
            ),
            "export default function AuthRegister({ title }: AuthRegisterProps) {",
        ),
        ProjectFrontend::Jsx => (
            concat!(
                "import { FieldError, Form, Link, redirect } from \"@apizero/react\";\n",
                "import { RegisterInputFields } from \"../../generated/contracts.js\";\n",
                "import { home, login, register } from \"../../generated/routes.js\";",
            ),
            "export default function AuthRegister({ title }) {",
        ),
    };
    format!("{imports}\n\n{signature}\n{AUTH_REGISTER_PAGE_BODY}")
}

const AUTH_FORGOT_PASSWORD_PAGE_BODY: &str = r#"  const [notice, setNotice] = useState("");
  return (
    <main className="auth-page">
      <h1>{title}</h1>
      {notice ? <p role="status">{notice}</p> : null}
      <Form
        action={passwordReset.store}
        initialValues={{ email: "" }}
        fields={PasswordResetInputFields}
        onSuccess={(result) => setNotice(result.message)}
      >
        {(form) => (
          <>
            <label htmlFor="reset-email">邮箱</label>
            <input id="reset-email" type="email" autoComplete="email" {...form.field("email")} />
            <FieldError errors={form.errors} name="email" />

            <button type="submit" disabled={form.processing}>
              {form.processing ? "发送中…" : "发送重置说明"}
            </button>
          </>
        )}
      </Form>
      <p>
        想起密码了？<Link href={login.show()}>返回登录</Link>
      </p>
    </main>
  );
}
"#;

fn auth_forgot_password_page_template(frontend: ProjectFrontend) -> String {
    let (imports, signature) = match frontend {
        ProjectFrontend::Tsx => (
            concat!(
                "import { useState } from \"react\";\n",
                "import { FieldError, Form, Link } from \"@apizero/react\";\n",
                "import type { AuthForgotPasswordProps } from \"../../generated/contracts.js\";\n",
                "import { PasswordResetInputFields } from \"../../generated/contracts.js\";\n",
                "import { login, routes } from \"../../generated/routes.js\";\n",
                "\n",
                "const passwordReset = routes[\"password-reset\"];",
            ),
            "export default function AuthForgotPassword({ title }: AuthForgotPasswordProps) {",
        ),
        ProjectFrontend::Jsx => (
            concat!(
                "import { useState } from \"react\";\n",
                "import { FieldError, Form, Link } from \"@apizero/react\";\n",
                "import { PasswordResetInputFields } from \"../../generated/contracts.js\";\n",
                "import { login, routes } from \"../../generated/routes.js\";\n",
                "\n",
                "const passwordReset = routes[\"password-reset\"];",
            ),
            "export default function AuthForgotPassword({ title }) {",
        ),
    };
    format!("{imports}\n\n{signature}\n{AUTH_FORGOT_PASSWORD_PAGE_BODY}")
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

fn page_template(component: &str, props: &str, depth: usize, frontend: ProjectFrontend) -> String {
    let contracts = format!("{}generated/contracts.js", "../".repeat(depth));
    if frontend == ProjectFrontend::Jsx {
        return format!(
            r"export default function {component}({{ title }}) {{
  return (
    <main>
      <h1>{{title}}</h1>
    </main>
  );
}}
"
        );
    }
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

fn island_template(component: &str, frontend: ProjectFrontend) -> String {
    if frontend == ProjectFrontend::Jsx {
        return format!(
            r#"import {{ useState }} from "react";

export default function {component}({{ initialCount = 0 }}) {{
  const [count, setCount] = useState(initialCount);
  return <button type="button" onClick={{() => setCount((value) => value + 1)}}>{{count}}</button>;
}}
"#
        );
    }
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

    /// Overwrite a file unconditionally (used to patch existing managed files
    /// such as `Cargo.toml` that have no marker regions).
    fn set(&mut self, relative: impl Into<PathBuf>, content: String) -> Result<(), ScaffoldError> {
        let relative = safe_relative(relative.into())?;
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
                .database(Some(ProjectDatabase::Sqlite))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();

        assert!(root.join("src/main.rs").is_file());
        assert!(root.join("src/bin/phoenix-manage.rs").is_file());
        // Runtime settings converged into `.env`; config/ holds Feature TOML only.
        assert!(!root.join("config/app.toml").exists());
        assert!(!root.join("config/database.toml").exists());
        assert!(!root.join("config/schemas").exists());
        assert!(!root.join("taplo.toml").exists());
        assert!(!root.join("deploy").exists());
        let env_example = fs::read_to_string(root.join(".env.example")).unwrap();
        assert!(env_example.contains("DATABASE_URL=sqlite:storage/app.sqlite"));
        assert!(env_example.contains("APP_ADDR=127.0.0.1:3000"));
        assert!(env_example.contains("VITE_DEV_URL="));
        assert!(env_example.contains("PHOENIX_LOG="));
        assert!(root.join("app/commands/mod.rs").is_file());
        assert!(root.join("config/mod.rs").is_file());
        assert!(root.join("database/seeders/mod.rs").is_file());
        assert!(root.join("routes/web.rs").is_file());
        assert!(root.join("views/pages/home.tsx").is_file());
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("crates/phoenix"));
        assert!(manifest.contains("default-run = \"phoenix-cli-new-"));
        assert!(manifest.contains("default = [\"sqlite\"]"));
        assert!(manifest.contains("sqlite = [\"database\", \"phoenix/sqlite\"]"));
        assert!(manifest.contains("features = [\"migration\", \"serde\", \"sqlite\"]"));
        assert!(manifest.contains("tls = [\"phoenix/tls\"]"));
        assert!(manifest.contains("websocket = [\"phoenix/websocket\"]"));
        assert!(manifest.contains("sse = [\"phoenix/sse\"]"));
        assert!(manifest.contains("default-features = false"));
        assert!(manifest.contains("opt-level = \"z\""));
        assert!(manifest.contains("strip = \"symbols\""));
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
        assert!(application.contains("StateMiddleware::new(renderer.clone())"));
        assert!(application.contains("RendererConfig::production"));
        assert!(application.contains("renderer.warm_up().await"));
        assert!(
            application
                .contains("let vite_dev_server = std::env::var_os(\"PHOENIX_VITE_DEV\").is_some()")
        );
        assert!(application.contains("RendererConfig::node(\"public/ssr/renderer.js\")"));
        let home_controller =
            fs::read_to_string(root.join("app/controllers/home_controller.rs")).unwrap();
        assert!(home_controller.contains("get::<NodeRenderer>().cloned()"));
        assert!(home_controller.contains("respond_with_renderer(request, &renderer).await"));
        let home_route = fs::read_to_string(root.join("routes/web.rs")).unwrap();
        assert!(home_route.contains(".get(\"/\", HomeController::index)"));
        assert!(home_route.contains(".name(\"demo.spa\")"));
        assert!(home_route.contains(".name(\"demo.ssr\")"));
        assert!(home_route.contains(".name(\"demo.islands\")"));
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["check", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
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
    fn default_project_is_islands_without_database_or_git() {
        let root = temporary_directory("default-options");
        let options = NewProjectOptions::new(&root)
            .dependencies(DependencySource::Local(framework_root()))
            .install_dependencies(false);
        assert!(!options.initialize_git);
        assert_eq!(options.render_mode, ProjectRenderMode::Islands);
        assert_eq!(options.database, None);
        assert_eq!(options.frontend, ProjectFrontend::Tsx);
        create_project(&options).unwrap();

        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("default = []"));
        assert!(!cargo.contains("toasty ="));
        assert!(!root.join("config/database.toml").exists());
        assert!(!root.join("src/bin/phoenix-manage.rs").exists());
        let home_controller =
            fs::read_to_string(root.join("app/controllers/home_controller.rs")).unwrap();
        // The chosen mode drives the home page; every mode has a demo action.
        assert!(home_controller.contains("Page::new(\n            \"home\""));
        assert!(home_controller.contains(".islands();"));
        assert!(home_controller.contains("Page::new(\"demo/spa\", serde_json::json!({})).spa()"));
        assert!(home_controller.contains("DemoSsrProps { rendered_at }).ssr()"));
        assert!(
            home_controller
                .contains("Page::new(\"demo/islands\", serde_json::json!({})).islands()")
        );
        // One demo page per render mode, plus the hydrated counter island.
        let spa = fs::read_to_string(root.join("views/pages/demo/spa.tsx")).unwrap();
        assert!(spa.contains("useState"));
        let ssr = fs::read_to_string(root.join("views/pages/demo/ssr.tsx")).unwrap();
        assert!(ssr.contains("renderedAt"));
        let islands = fs::read_to_string(root.join("views/pages/demo/islands.tsx")).unwrap();
        assert!(islands.contains("client:load"));
        assert!(root.join("views/islands/counter.tsx").is_file());
        let home = fs::read_to_string(root.join("views/pages/home.tsx")).unwrap();
        assert!(home.contains("<Link href={demo.spa()}"));
        assert!(home.contains("<Link href={demo.ssr()}"));
        assert!(home.contains("<Link href={demo.islands()}"));
        assert!(!root.join(".git").exists());

        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["check", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_options_select_jsx_tailwind_and_database() {
        let root = temporary_directory("options");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Pgsql))
                .render_mode(ProjectRenderMode::Ssr)
                .frontend(ProjectFrontend::Jsx)
                .tailwind(true)
                .install_dependencies(false),
        )
        .unwrap();

        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("default = [\"pgsql\"]"));
        assert!(cargo.contains("\"postgresql\""));
        assert!(root.join("views/pages/home.jsx").is_file());
        assert!(!root.join("views/pages/home.tsx").exists());
        assert!(
            fs::read_to_string(root.join("app/controllers/home_controller.rs"))
                .unwrap()
                .contains(".ssr()")
        );
        assert!(
            fs::read_to_string(root.join("package.json"))
                .unwrap()
                .contains("@tailwindcss/vite")
        );
        assert!(
            fs::read_to_string(root.join("views/styles.css"))
                .unwrap()
                .starts_with("@import \"tailwindcss\";")
        );
        assert!(
            fs::read_to_string(root.join("vite.config.ts"))
                .unwrap()
                .contains("tailwindcss()")
        );
        let generator = ProjectGenerator::discover(&root).unwrap();
        generator
            .page("reports/index", GenerateOptions::default())
            .unwrap();
        generator
            .island("LikeButton", GenerateOptions::default())
            .unwrap();
        assert!(root.join("views/pages/reports/index.jsx").is_file());
        assert!(root.join("views/islands/like-button.jsx").is_file());
        // The scaffolded demo island is already there in the selected frontend.
        assert!(root.join("views/islands/counter.jsx").is_file());
        assert!(
            !fs::read_to_string(root.join("views/pages/reports/index.jsx"))
                .unwrap()
                .contains("import type")
        );
        generator.auth(GenerateOptions::default()).unwrap();
        assert!(root.join("views/pages/auth/login.jsx").is_file());
        assert!(
            !fs::read_to_string(root.join("views/pages/auth/login.jsx"))
                .unwrap()
                .contains("import type")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn all_database_option_writes_all_toasty_drivers() {
        let root = temporary_directory("all-databases");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::All))
                .install_dependencies(false),
        )
        .unwrap();
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains(
            "toasty = { version = \"0.8\", default-features = false, features = [\"migration\", \"mysql\", \"postgresql\", \"serde\", \"sqlite\"] }"
        ));
        assert!(cargo.contains("all-databases = [\"database\", \"phoenix/sqlite\", \"phoenix/pgsql\", \"phoenix/mysql\"]"));
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["check", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_dependencies_use_plain_versions_not_npmjs_tarballs() {
        let (phoenix, react, react_ssr, vite) =
            framework_dependency_pins(&DependencySource::Registry, &[]).unwrap();
        for pin in [&phoenix, &react, &react_ssr, &vite] {
            assert!(
                !pin.contains("registry.npmjs.org"),
                "registry pins must not bypass the user's npm mirror: {pin}"
            );
        }
        assert_eq!(react, format!("^{APIZERO_REACT_VERSION}"));
        assert_eq!(react_ssr, format!("^{APIZERO_REACT_SSR_VERSION}"));
        assert_eq!(vite, format!("^{APIZERO_VITE_VERSION}"));
        assert!(phoenix.contains(&format!("version = \"{PHOENIXRS_VERSION}\"")));
        assert!(!phoenix.contains("features = ["));

        let (phoenix, ..) = framework_dependency_pins(
            &DependencySource::Registry,
            &[ProjectFeature::Captcha, ProjectFeature::Pay],
        )
        .unwrap();
        assert!(phoenix.contains("features = [\"captcha\", \"pay\"]"));
    }

    #[test]
    fn feature_selection_generates_config_and_assembly() {
        let root = temporary_directory("features");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Sqlite))
                .features(vec![
                    ProjectFeature::Captcha,
                    ProjectFeature::Pay,
                    ProjectFeature::Notify,
                ])
                .install_dependencies(false),
        )
        .unwrap();

        let cargo_manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo_manifest.contains("features = [\"captcha\", \"pay\", \"notify\"]"));

        let pay_config = fs::read_to_string(root.join("config/pay.toml")).unwrap();
        assert!(pay_config.contains("${PAY_WECHAT_API_V3_KEY}"));
        assert!(root.join("config/captcha.toml").is_file());
        assert!(root.join("config/notify.toml").is_file());

        let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib.contains("pub fn features()"));
        assert!(lib.contains("CaptchaFeature::new()"));
        assert!(lib.contains("PayFeature::new(pay_manager()?)"));
        assert!(lib.contains("NotifyFeature::new()"));
        assert!(lib.contains("load_feature_config(\"pay\")"));
        assert!(lib.contains("pub fn notifier()"));
        // Webhooks stay outside CSRF; business routes keep it via the scope.
        assert!(lib.contains(".scoped(RouteGroup::new().middleware(Csrf))"));
        assert!(lib.contains(".merge(features()?.into_routes())"));

        let manage = fs::read_to_string(root.join("src/bin/phoenix-manage.rs")).unwrap();
        assert!(manage.contains("features.into_parts().migrations"));

        let env_example = fs::read_to_string(root.join(".env.example")).unwrap();
        assert!(env_example.contains("PAY_WECHAT_API_V3_KEY"));
        let stored = fs::read_to_string(root.join(".phoenix")).unwrap();
        assert!(stored.contains("features=captcha,pay,notify"));
        let readme = fs::read_to_string(root.join("README.md")).unwrap();
        assert!(readme.contains("已启用 Feature"));

        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["check", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stored_feature_options_survive_update_core() {
        let root = temporary_directory("features-update");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .features(vec![ProjectFeature::Captcha])
                .install_dependencies(false),
        )
        .unwrap();
        // User values in feature config files must survive `px update`.
        fs::write(root.join("config/captcha.toml"), "length = 6\n").unwrap();

        let generator = ProjectGenerator::discover(&root).unwrap();
        let _ = generator
            .update_core(
                &UpdateProjectOptions::new()
                    .dependencies(DependencySource::Local(framework_root()))
                    .install_dependencies(false),
            )
            .unwrap();

        let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib.contains("CaptchaFeature::new()"));
        let cargo_manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo_manifest.contains("features = [\"captcha\"]"));
        assert_eq!(
            fs::read_to_string(root.join("config/captcha.toml")).unwrap(),
            "length = 6\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn make_command_registers_async_handler() {
        let root = temporary_directory("command");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Sqlite))
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
                .database(Some(ProjectDatabase::Sqlite))
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
    fn relations_and_factories_generate_a_compiling_slice() {
        let root = temporary_directory("relations");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Sqlite))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();

        // The parent side: one line to say "a user has many posts".
        generator
            .model(
                "User",
                ModelOptions {
                    migration: true,
                    factory: true,
                    relations: vec![Relation {
                        kind: RelationKind::HasMany,
                        target: "Post".to_owned(),
                    }],
                    ..ModelOptions::default()
                },
            )
            .unwrap();
        // The child side: one line to say "a post belongs to a user". No
        // foreign key, no key mapping, no table name.
        generator
            .model(
                "Post",
                ModelOptions {
                    migration: true,
                    factory: true,
                    relations: vec![Relation {
                        kind: RelationKind::BelongsTo,
                        target: "User".to_owned(),
                    }],
                    ..ModelOptions::default()
                },
            )
            .unwrap();

        let post = fs::read_to_string(root.join("app/models/post.rs")).unwrap();
        assert!(post.contains("#[model]"), "{post}");
        assert!(post.contains("#[belongs_to]"), "{post}");
        assert!(post.contains("pub user: Deferred<User>"), "{post}");
        assert!(
            !post.contains("user_id"),
            "the foreign key is a convention, not something to write: {post}"
        );
        assert!(
            !post.contains("#[key]") && !post.contains("#[table"),
            "neither is the key or the table name: {post}"
        );

        let user = fs::read_to_string(root.join("app/models/user.rs")).unwrap();
        assert!(user.contains("#[has_many]"), "{user}");
        assert!(user.contains("pub posts: Deferred<Vec<Post>>"), "{user}");

        // The child factory takes its parent's key; the parent factory does not.
        let factory = fs::read_to_string(root.join("database/seeders/post_factory.rs")).unwrap();
        assert!(factory.contains("|f, user_id: i64|"), "{factory}");
        assert!(factory.contains(".user_id(user_id)"), "{factory}");
        assert!(
            factory.contains(r#"#![cfg(feature = "factory")]"#),
            "a factory never reaches a release build: {factory}"
        );
        let factory = fs::read_to_string(root.join("database/seeders/user_factory.rs")).unwrap();
        assert!(factory.contains("|f|"), "{factory}");

        let seeders = fs::read_to_string(root.join("database/seeders/mod.rs")).unwrap();
        assert!(seeders.contains("pub mod post_factory;"), "{seeders}");
        assert!(seeders.contains("pub mod user_factory;"), "{seeders}");

        // The real assertion: all of it compiles, with factories on and off.
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        for features in [None, Some("factory")] {
            let mut command = Command::new(&cargo);
            command.args(["check", "--quiet"]).current_dir(&root);
            if let Some(features) = features {
                command.args(["--features", features]);
            }
            assert!(
                command.status().unwrap().success(),
                "generated project must compile with features {features:?}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn make_auth_requires_a_database() {
        let root = temporary_directory("auth-no-db");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();
        // No driver was selected at `px new`, so the persistent User model has
        // nowhere to live: refuse with a clear, actionable error and write nothing.
        assert!(matches!(
            generator.auth(GenerateOptions::default()),
            Err(ScaffoldError::AuthRequiresDatabase)
        ));
        assert!(!root.join("routes/auth.rs").exists());
        assert!(!root.join("app/models/user.rs").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one assertion per generated file")]
    fn make_auth_generates_a_working_authentication_slice() {
        let root = temporary_directory("auth");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Sqlite))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();
        let generator = ProjectGenerator::discover(&root).unwrap();
        generator.auth(GenerateOptions::default()).unwrap();

        for path in [
            "routes/auth.rs",
            "app/controllers/auth_controller.rs",
            "app/models/user.rs",
            "app/requests/login_input.rs",
            "app/requests/register_input.rs",
            "app/requests/password_reset_input.rs",
            "app/resources/auth_session_resource.rs",
            "app/resources/auth_message_resource.rs",
            "app/props/auth/auth_login_props.rs",
            "app/props/auth/auth_register_props.rs",
            "app/props/auth/auth_forgot_password_props.rs",
            "views/pages/auth/login.tsx",
            "views/pages/auth/register.tsx",
            "views/pages/auth/forgot-password.tsx",
        ] {
            assert!(root.join(path).is_file(), "missing {path}");
        }

        // Persistent User model + migration registered like `px make:model`.
        let user_model = fs::read_to_string(root.join("app/models/user.rs")).unwrap();
        assert!(user_model.contains("#[derive(Clone, Debug, Model)]"));
        assert!(user_model.contains("#[unique]"));
        assert!(user_model.contains("pub email: String"));
        assert!(user_model.contains("pub password_hash: String"));
        assert!(user_model.contains("pub created_at: String"));
        let models = fs::read_to_string(root.join("app/models/mod.rs")).unwrap();
        assert!(models.contains("// phoenix:model: User"));
        assert!(models.contains("pub use user::User;"));
        let migrations = fs::read_to_string(root.join("database/migrations/mod.rs")).unwrap();
        assert!(migrations.contains("create_users_table"));
        // The Argon2id (`password`) and guard (`auth`) facade features are on by default.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let default_line = cargo
            .lines()
            .find(|line| line.trim_start().starts_with("default ="))
            .expect("default features line");
        assert!(default_line.contains("\"sqlite\""));
        assert!(default_line.contains("\"password\""));
        assert!(default_line.contains("\"auth\""));

        let routes = fs::read_to_string(root.join("routes/auth.rs")).unwrap();
        assert!(routes.contains(".name(\"login.show\")"));
        assert!(routes.contains(".name(\"login.store\")"));
        assert!(routes.contains(".name(\"logout.store\")"));
        assert!(routes.contains(".name(\"register.store\")"));
        assert!(routes.contains(".name(\"password-reset.store\")"));
        assert!(routes.contains(".action::<LoginInput, AuthSessionResource>()"));
        assert!(routes.contains(".action::<RegisterInput, AuthSessionResource>()"));
        assert!(routes.contains(".action::<PasswordResetInput, AuthMessageResource>()"));
        assert!(routes.contains("RateLimit::new(RateLimitConfig {"));
        assert!(routes.contains("phoenix-captcha"), "captcha assembly notes");

        let controller =
            fs::read_to_string(root.join("app/controllers/auth_controller.rs")).unwrap();
        // Credentials go through Argon2id + the phoenix-auth guard, never plaintext.
        assert!(
            controller
                .contains("use phoenix::auth::{AuthError, AuthGuard, AuthUser, UserProvider}")
        );
        assert!(controller.contains("impl UserProvider for DatabaseUserProvider"));
        assert!(controller.contains("guard.attempt(&input.email, &input.password)"));
        assert!(controller.contains("Password::hash(&input.password)"));
        assert!(controller.contains("StatusCode::UNPROCESSABLE_ENTITY"));
        assert!(controller.contains("Validated::<Json<LoginInput>>::from_request"));
        assert!(controller.contains("CaptchaProtected"));
        assert!(!controller.contains("DemoUser"), "no in-memory demo store");
        assert!(
            !controller.contains("user.password == input.password"),
            "no plaintext password comparison"
        );
        let controllers = fs::read_to_string(root.join("app/controllers/mod.rs")).unwrap();
        assert!(controllers.contains("pub use auth_controller::AuthController;"));
        let requests = fs::read_to_string(root.join("app/requests/login_input.rs")).unwrap();
        assert!(requests.contains("#[phoenix::contract(input)]"));
        assert!(requests.contains("impl Validate for LoginInput"));
        let props = fs::read_to_string(root.join("app/props/auth/auth_login_props.rs")).unwrap();
        assert!(props.contains("#[phoenix::contract(page, page = \"auth/login\")]"));

        let login_page = fs::read_to_string(root.join("views/pages/auth/login.tsx")).unwrap();
        assert!(login_page.contains("<Form"));
        assert!(login_page.contains("form.field(\"email\")"));
        assert!(login_page.contains("form.field(\"password\")"));
        assert!(login_page.contains("<FieldError errors={form.errors} name=\"password\" />"));
        assert!(login_page.contains("fields={LoginInputFields}"));
        assert!(login_page.contains("CaptchaImage"));
        let reset_page =
            fs::read_to_string(root.join("views/pages/auth/forgot-password.tsx")).unwrap();
        assert!(reset_page.contains("fields={PasswordResetInputFields}"));
        assert!(reset_page.contains("routes[\"password-reset\"]"));

        assert!(matches!(
            generator.auth(GenerateOptions::default()),
            Err(ScaffoldError::AlreadyExists(_))
        ));
        generator.auth(GenerateOptions { force: true }).unwrap();
        // The forced rebuild must not stack a second users migration. Assert on
        // the registration itself rather than counting substrings: one
        // migration is named three times in the registry (marker comment,
        // `pub mod`, and the `all()` entry).
        let migrations = fs::read_to_string(root.join("database/migrations/mod.rs")).unwrap();
        assert_eq!(
            migrations.matches("pub mod m_").count(),
            1,
            "exactly one migration is registered:\n{migrations}"
        );
        let migration_files = fs::read_dir(root.join("database/migrations"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != "mod.rs")
            .count();
        assert_eq!(migration_files, 1, "and exactly one migration file exists");

        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["check", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_core_refreshes_framework_files_only() {
        let root = temporary_directory("update-core");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .initialize_git(false)
                .install_dependencies(false),
        )
        .unwrap();

        let route_before = fs::read_to_string(root.join("routes/web.rs")).unwrap();
        let page_before = fs::read_to_string(root.join("views/pages/home.tsx")).unwrap();
        fs::write(root.join("src/lib.rs"), "// stale core\n").unwrap();
        fs::write(
            root.join("routes/web.rs"),
            format!("{route_before}\n// business marker\n"),
        )
        .unwrap();

        let generator = ProjectGenerator::discover(&root).unwrap();
        let changed = generator
            .update_core(
                &UpdateProjectOptions::new()
                    .dependencies(DependencySource::Local(framework_root()))
                    .install_dependencies(false),
            )
            .unwrap();
        assert!(
            changed.iter().any(|path| path.ends_with("src/lib.rs")),
            "expected src/lib.rs to be refreshed"
        );

        let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("let vite_dev_server = std::env::var_os(\"PHOENIX_VITE_DEV\").is_some()")
        );
        assert!(!lib.contains("// stale core"));
        let route_after = fs::read_to_string(root.join("routes/web.rs")).unwrap();
        assert!(route_after.contains("// business marker"));
        assert_eq!(
            fs::read_to_string(root.join("views/pages/home.tsx")).unwrap(),
            page_before
        );
        let options = fs::read_to_string(root.join(".phoenix")).unwrap();
        assert!(options.contains("render_mode=islands"));
        assert!(options.contains("database=none"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generators_refuse_overwrites_and_path_traversal() {
        let root = temporary_directory("safety");
        create_project(
            &NewProjectOptions::new(&root)
                .dependencies(DependencySource::Local(framework_root()))
                .database(Some(ProjectDatabase::Sqlite))
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
