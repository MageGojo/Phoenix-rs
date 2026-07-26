use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process::{Command, ExitCode},
};

use phoenix_cli::{
    ControllerOptions, DependencySource, DevConfig, DevSupervisor, GenerateOptions, ModelOptions,
    NewProjectOptions, ProjectDatabase, ProjectFeature, ProjectFrontend, ProjectGenerator,
    ProjectRenderMode, UpdateProjectOptions, parse_feature_list, release_build, release_install,
    release_rollback, release_status, scaffold_project,
};

const HELP: &str = r"Phoenix-rs application CLI (px)

Install: cargo install px-cli
         cargo install --git https://github.com/MageGojo/Phoenix-rs px-cli
         cargo install --path crates/phoenix-cli

Usage:
  px new [project] [--render-mode islands|spa|ssr] [--database sqlite|pgsql|mysql|all]
                   [--tailwind] [--git] [--frontend tsx|jsx]
                   [--feature captcha,pay,notify] [--no-features]
                   [--framework-path <path>] [--no-install] [--no-git]
  px update [--framework-path <path>] [--no-install] [--dry-run]
  px dev
  px migrate
  px status
  px rollback [--step <count>]
  px fresh [--seed]
  px seed
  px make:auth [--force]
  px make:controller <name> [--resource] [--route] [--force]
  px make:model <name> [--all] [--migration] [--controller] [--resource]
                            [--request] [--api-resource] [--page] [--force]
  px make:migration <name> [--force]
  px make:request <name> [--force]
  px make:resource <name> [--force]
  px make:middleware <name> [--force]
  px make:page <path> [--force]
  px make:island <name> [--force]
  px make:command <name> [--force]
  px schedule:run
  px schedule:work
  px list
  px release [--version <v>] [--output <dir>] [--tarball] [--bin <name>]
               [--skip-npm] [--skip-types] [--target <triple>]
  px release:install --tarball <path> | --path <dir> [--deploy-root <dir>]
                     [--version <v>] [--skip-migrate] [--no-switch]
                     [--restart-cmd <shell>] [--dry-run]
  px release:rollback [--deploy-root <dir>] [--to <version>] [--steps <n>]
                      [--restart-cmd <shell>] [--skip-restart] [--dry-run]
  px release:status [--deploy-root <dir>] [--json]

Examples:
  px new my-app
  px update
  px migrate
  px rollback --step 2
  px fresh --seed
  px make:model Post --all
  px make:auth
  px make:controller Admin/ReportController --resource
  px make:page posts/index
  px make:command Update
  px release --version 0.1.0 --tarball
  px release:install --tarball ./app-0.1.0.tar.gz --version 0.1.0
  px release:status
";

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("px failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(raw: Vec<OsString>) -> Result<(), String> {
    let mut arguments = raw
        .into_iter()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if arguments.is_empty() || matches!(arguments[0].as_str(), "help" | "--help" | "-h") {
        print!("{HELP}");
        return Ok(());
    }
    let command = arguments.remove(0);
    match command.as_str() {
        "dev" => dev(arguments).await,
        "new" => new_project(arguments),
        "update" => update_project(&arguments),
        "migrate" => database_command("migrate", &no_options(&arguments)?),
        "status" => database_command("status", &no_options(&arguments)?),
        "rollback" => database_command("rollback", &rollback_options(&arguments)?),
        "fresh" => database_command("fresh", &fresh_options(&arguments)?),
        "seed" => database_command("seed", &no_options(&arguments)?),
        "make:auth" => make_auth(arguments),
        "make:controller" => make_controller(arguments),
        "make:model" => make_model(arguments),
        "make:migration" => make_simple(arguments, |generator, name, options| {
            generator.migration(name, options)
        }),
        "make:request" => make_simple(arguments, |generator, name, options| {
            generator.request(name, options)
        }),
        "make:resource" => make_simple(arguments, |generator, name, options| {
            generator.resource(name, options)
        }),
        "make:middleware" => make_simple(arguments, |generator, name, options| {
            generator.middleware(name, options)
        }),
        "make:page" => make_simple(arguments, |generator, name, options| {
            generator.page(name, options)
        }),
        "make:island" => make_simple(arguments, |generator, name, options| {
            generator.island(name, options)
        }),
        "make:command" => make_simple(arguments, |generator, name, options| {
            generator.command(name, options)
        }),
        "schedule:run" | "schedule:work" => app_console_command(&command, &arguments),
        "list" => {
            require_empty(&arguments)?;
            print!("{HELP}");
            Ok(())
        }
        "release" | "release:build" => release_build(arguments),
        "release:install" => release_install(arguments),
        "release:rollback" => release_rollback(arguments),
        "release:status" => release_status(arguments),
        _ => Err(format!("unknown command `{command}`\n\n{HELP}")),
    }
}

async fn dev(arguments: Vec<String>) -> Result<(), String> {
    require_empty(&arguments)?;
    let generator = current_generator()?;
    if !generator.root().join("node_modules").is_dir() {
        return Err("JavaScript dependencies are missing; run `npm install` first".to_owned());
    }

    println!("Phoenix development environment");
    println!("  application: {}", generator.root().display());
    println!("  build:       npm run build:client && npm run build:ssr (automatic)");
    println!("  backend:     cargo run -- serve  (restarts after Rust/React changes)");
    println!("  frontend:    npm run dev -- --strictPort");
    println!("Press Ctrl-C to stop both processes.\n");

    DevSupervisor::new(DevConfig::default().working_directory(generator.root()))
        .run()
        .await
        .map_err(|error| error.to_string())
}

fn database_command(command: &str, arguments: &[String]) -> Result<(), String> {
    let generator = current_generator()?;
    let manager = generator.root().join("src/bin/phoenix-manage.rs");
    if !manager.is_file() {
        return Err(format!(
            "{} is missing; add the Phoenix management binary before running database commands",
            manager.display()
        ));
    }

    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .args(["run", "--quiet", "--bin", "phoenix-manage", "--", command])
        .args(arguments)
        .current_dir(generator.root())
        .status()
        .map_err(|error| format!("failed to start the project management binary: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "project management command `{command}` exited with {status}"
        ))
    }
}

/// Forward a command (e.g. `schedule:run` / `schedule:work`) to the
/// application's console binary: `cargo run -- <command> [args...]`.
///
/// The application must have registered the command — for the scheduler via
/// `phoenix_schedule::console_commands(...)` in its `Console` setup.
fn app_console_command(command: &str, arguments: &[String]) -> Result<(), String> {
    let generator = current_generator()?;
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .args(["run", "--quiet", "--", command])
        .args(arguments)
        .current_dir(generator.root())
        .status()
        .map_err(|error| format!("failed to start the application console: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "application console command `{command}` exited with {status}"
        ))
    }
}

fn no_options(arguments: &[String]) -> Result<Vec<String>, String> {
    require_empty(arguments)?;
    Ok(Vec::new())
}

fn rollback_options(arguments: &[String]) -> Result<Vec<String>, String> {
    let mut steps = 1_usize;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--step" {
            index += 1;
            let value = arguments.get(index).ok_or("--step requires a count")?;
            steps = parse_steps(value)?;
        } else if let Some(value) = argument.strip_prefix("--step=") {
            steps = parse_steps(value)?;
        } else {
            return Err(format!("unknown rollback option `{argument}`"));
        }
        index += 1;
    }
    Ok(vec![steps.to_string()])
}

fn parse_steps(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|steps| *steps > 0)
        .ok_or_else(|| "rollback step count must be a positive integer".to_owned())
}

fn fresh_options(arguments: &[String]) -> Result<Vec<String>, String> {
    match arguments {
        [] => Ok(Vec::new()),
        [option] if option == "--seed" => Ok(vec![option.clone()]),
        [option] => Err(format!("unknown fresh option `{option}`")),
        _ => Err(format!("unexpected arguments: {}", arguments.join(" "))),
    }
}

// The body is a flat flag/wizard surface; the length is steps, not logic.
#[allow(clippy::too_many_lines)]
fn new_project(mut arguments: Vec<String>) -> Result<(), String> {
    let style = Style::detect();
    let interactive = io::stdin().is_terminal();
    let named = arguments
        .first()
        .is_some_and(|argument| !argument.starts_with('-'));
    let target = if named {
        arguments.remove(0)
    } else if interactive {
        style.banner();
        prompt_name(&style)?
    } else {
        return Err("a project name is required; try: px new my-app".to_owned());
    };
    let flags = arguments;
    let mut options = NewProjectOptions::new(&target);
    let mut render_mode_set = false;
    let mut database_set = false;
    let mut tailwind_set = false;
    let mut git_set = false;
    let mut features_set = false;
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--no-install" => options.install_dependencies = false,
            "--no-git" => {
                options.initialize_git = false;
                git_set = true;
            }
            "--git" => {
                options.initialize_git = true;
                git_set = true;
            }
            "--tailwind" | "--tailwindcss" => {
                options.tailwind = true;
                tailwind_set = true;
            }
            "--no-tailwind" => {
                options.tailwind = false;
                tailwind_set = true;
            }
            "--render-mode" => {
                index += 1;
                let value = flags.get(index).ok_or("--render-mode requires a value")?;
                options.render_mode = value.parse::<ProjectRenderMode>()?;
                render_mode_set = true;
            }
            "--database" => {
                index += 1;
                let value = flags.get(index).ok_or("--database requires a value")?;
                options.database = Some(value.parse::<ProjectDatabase>()?);
                database_set = true;
            }
            "--no-database" => {
                options.database = None;
                database_set = true;
            }
            "--feature" | "--features" => {
                index += 1;
                let value = flags.get(index).ok_or("--feature requires a value")?;
                let mut parsed = parse_feature_list(value)?;
                for feature in parsed.drain(..) {
                    if !options.features.contains(&feature) {
                        options.features.push(feature);
                    }
                }
                options.features.sort_unstable();
                features_set = true;
            }
            "--no-features" => {
                options.features.clear();
                features_set = true;
            }
            "--frontend" => {
                index += 1;
                let value = flags.get(index).ok_or("--frontend requires a value")?;
                options.frontend = value.parse::<ProjectFrontend>()?;
            }
            "--framework-path" => {
                index += 1;
                let path = flags.get(index).ok_or("--framework-path requires a path")?;
                options.dependencies = DependencySource::Local(PathBuf::from(path));
            }
            flag => return Err(format!("unknown new-project option `{flag}`")),
        }
        index += 1;
    }
    if interactive {
        if named {
            style.banner();
            style.done_line("项目名", &target);
        }
        if !render_mode_set {
            options.render_mode = prompt_render_mode(&style)?;
        }
        if !database_set {
            options.database = prompt_database(&style)?;
        }
        if !tailwind_set {
            options.tailwind = prompt_yes_no(
                &style,
                "Tailwind CSS",
                &[
                    ("0", "否", "仅使用标准 CSS"),
                    ("1", "是", "启用 Tailwind CSS v4（@tailwindcss/vite）"),
                ],
                "0",
            )?;
        }
        if !features_set {
            options.features = prompt_features(&style)?;
        }
        if !git_set {
            options.initialize_git = prompt_yes_no(
                &style,
                "初始化 Git 仓库",
                &[
                    ("0", "否", "跳过 git init"),
                    ("1", "是", "在新项目里执行 git init"),
                ],
                "0",
            )?;
        }
        print_summary(&style, &target, &options);
        if !prompt_yes_no(
            &style,
            "确认创建",
            &[
                ("1", "创建", "按上面的配置生成项目"),
                ("0", "取消", "退出向导，不写任何文件"),
            ],
            "1",
        )? {
            println!("{}", style.dim("已取消，未写任何文件。"));
            return Ok(());
        }
        println!();
    }
    run_creation(&style, &target, &options)
}

/// Scaffold + npm + git with grouped progress; npm failures explain how to
/// switch registries instead of aborting with a bare error.
fn run_creation(style: &Style, target: &str, options: &NewProjectOptions) -> Result<(), String> {
    let written = scaffold_project(options).map_err(|error| error.to_string())?;
    style.step_done(&format!("生成项目骨架 ✓（{written} 个文件）"));

    if options.initialize_git {
        match run_step("git", &["init", "--quiet"], &options.target) {
            StepOutcome::Ok => style.step_done("git init ✓"),
            StepOutcome::Missing => style.step_done("git init（未安装 git，已跳过）"),
            StepOutcome::Failed => return Err("git init 失败".to_owned()),
        }
    }
    if options.install_dependencies {
        style.step_start("npm install …");
        match run_step("npm", &["install"], &options.target) {
            StepOutcome::Ok => style.step_done("npm install ✓"),
            StepOutcome::Missing | StepOutcome::Failed => {
                println!();
                println!("{}", style.red("✗ npm install 失败。"));
                println!("  多为网络或镜像问题；国内环境可切换 npmmirror 后重试：");
                println!("    npm config set registry https://registry.npmmirror.com");
                println!("    cd {target} && npm install && npm run types");
                println!("  项目文件已全部生成，稍后重试安装即可，不影响已有代码。");
                return Err("npm install 未完成（见上方重试命令）".to_owned());
            }
        }
        style.step_start("生成契约类型 …");
        match run_step("npm", &["run", "types", "--silent"], &options.target) {
            StepOutcome::Ok => style.step_done("生成契约类型 ✓"),
            StepOutcome::Missing | StepOutcome::Failed => {
                println!();
                println!("{}", style.red("✗ 契约类型生成失败。"));
                println!("  可稍后在项目里重试：cd {target} && npm run types");
                return Err("npm run types 未完成".to_owned());
            }
        }
    }

    print_next_steps(style, target, options);
    Ok(())
}

enum StepOutcome {
    Ok,
    Missing,
    Failed,
}

fn run_step(program: &str, args: &[&str], cwd: &std::path::Path) -> StepOutcome {
    match Command::new(program).args(args).current_dir(cwd).status() {
        Ok(status) if status.success() => StepOutcome::Ok,
        Err(error) if error.kind() == io::ErrorKind::NotFound => StepOutcome::Missing,
        Ok(_) | Err(_) => StepOutcome::Failed,
    }
}

fn print_summary(style: &Style, target: &str, options: &NewProjectOptions) {
    println!();
    println!("{}", style.heading("◆ 配置汇总"));
    let rows = [
        ("项目名", target.to_owned()),
        (
            "渲染模式",
            render_mode_label(options.render_mode).to_owned(),
        ),
        (
            "数据库",
            options.database.map_or_else(
                || "无".to_owned(),
                |database| database_label(database).to_owned(),
            ),
        ),
        (
            "Tailwind",
            if options.tailwind { "是" } else { "否" }.to_owned(),
        ),
        (
            "Feature",
            if options.features.is_empty() {
                "无".to_owned()
            } else {
                options
                    .features
                    .iter()
                    .map(|feature| feature.key())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        (
            "Git",
            if options.initialize_git { "是" } else { "否" }.to_owned(),
        ),
        (
            "前端",
            match options.frontend {
                ProjectFrontend::Tsx => "TSX".to_owned(),
                ProjectFrontend::Jsx => "JSX".to_owned(),
            },
        ),
    ];
    for (label, value) in rows {
        // 全角冒号对齐即可，避免 CJK 宽度对不齐的问题。
        println!("  {}：{}", style.dim(label), value);
    }
    println!();
}

fn print_next_steps(style: &Style, target: &str, options: &NewProjectOptions) {
    println!();
    println!("{}", style.heading("◆ 下一步"));
    println!("  cd {target} && px dev");
    println!("  首页            http://127.0.0.1:3000/");
    println!("  渲染模式演示    /demo/spa · /demo/ssr · /demo/islands");
    if options.database.is_some() {
        println!("  数据库          .env 的 DATABASE_URL；迁移用 px migrate");
    }
    for feature in &options.features {
        println!(
            "  Feature {key:<8}config/{key}.toml（密钥放 .env）",
            key = feature.key()
        );
    }
    if !options.install_dependencies {
        println!("  依赖未安装      cd {target} && npm install && npm run types");
    }
    println!();
}

const fn render_mode_label(render_mode: ProjectRenderMode) -> &'static str {
    match render_mode {
        ProjectRenderMode::Islands => "Islands",
        ProjectRenderMode::Spa => "SPA",
        ProjectRenderMode::Ssr => "SSR",
    }
}

const fn database_label(database: ProjectDatabase) -> &'static str {
    match database {
        ProjectDatabase::Sqlite => "SQLite",
        ProjectDatabase::Pgsql => "PostgreSQL",
        ProjectDatabase::Mysql => "MySQL",
        ProjectDatabase::All => "全部驱动",
    }
}

fn update_project(arguments: &[String]) -> Result<(), String> {
    let mut options = UpdateProjectOptions::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--no-install" => options.install_dependencies = false,
            "--dry-run" => options.dry_run = true,
            "--framework-path" => {
                index += 1;
                let path = arguments
                    .get(index)
                    .ok_or("--framework-path requires a path")?;
                options.dependencies = DependencySource::Local(PathBuf::from(path));
            }
            flag => return Err(format!("unknown update option `{flag}`")),
        }
        index += 1;
    }

    let generator = current_generator()?;
    let changed = generator
        .update_core(&options)
        .map_err(|error| error.to_string())?;
    if changed.is_empty() {
        println!(
            "Core files already up to date at {}",
            generator.root().display()
        );
        return Ok(());
    }
    let label = if options.dry_run {
        "WOULD UPDATE"
    } else {
        "UPDATED"
    };
    for path in &changed {
        println!(
            "{label} {}",
            path.strip_prefix(generator.root())
                .unwrap_or(path)
                .display()
        );
    }
    if options.dry_run {
        println!("Dry run only; re-run without --dry-run to apply.");
    } else {
        println!(
            "Updated Phoenix core files in {} (business code left untouched).",
            generator.root().display()
        );
    }
    Ok(())
}

/// ANSI palette for the wizard; plain text when `NO_COLOR` is set or stdout is
/// not a terminal (CI and pipes degrade automatically).
struct Style {
    color: bool,
}

impl Style {
    fn detect() -> Self {
        Self {
            color: io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn heading(&self, text: &str) -> String {
        self.paint("1;36", text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn banner(&self) {
        println!();
        println!("{}", self.heading("◆ Phoenix-rs 新项目向导"));
        println!(
            "{}",
            self.dim("  回车接受默认值；所有问题都有对应命令行参数（px help）。")
        );
        println!();
    }

    /// `◇ 渲染模式 — 说明` step header.
    fn step_header(&self, title: &str, blurb: &str) {
        if blurb.is_empty() {
            println!("{} {}", self.paint("36", "◇"), self.paint("1", title));
        } else {
            println!(
                "{} {} {}",
                self.paint("36", "◇"),
                self.paint("1", title),
                self.dim(&format!("— {blurb}"))
            );
        }
    }

    /// `● 已选：Islands` confirmation line.
    fn done_line(&self, label: &str, value: &str) {
        println!("{} {}：{}", self.green("●"), self.dim(label), value);
        println!();
    }

    fn step_start(&self, text: &str) {
        println!("{} {}", self.paint("36", "◆"), text);
    }

    fn step_done(&self, text: &str) {
        println!("{} {}", self.green("◆"), text);
    }
}

fn prompt_name(style: &Style) -> Result<String, String> {
    style.step_header(
        "项目名",
        "同时作为目录名与 Cargo 包名（字母、数字、连字符）",
    );
    loop {
        print!("  名称: ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let name = prompt_input(None)?;
        if name.is_empty() {
            eprintln!("  ! 项目名不能为空，例如 my-app");
            continue;
        }
        style.done_line("项目名", &name);
        return Ok(name);
    }
}

fn prompt_render_mode(style: &Style) -> Result<ProjectRenderMode, String> {
    let mode = prompt_menu(
        style,
        "渲染模式",
        "React 页面如何送达浏览器",
        &[
            ("0", "Islands", "推荐——静态 HTML 里只水合交互组件，JS 最少"),
            ("1", "SPA", "整页浏览器渲染，适合后台与重交互应用"),
            ("2", "SSR", "每次请求服务端渲染完整 HTML，利于首屏与 SEO"),
        ],
        "0",
        |value| match value {
            "0" | "islands" | "island" => Ok(ProjectRenderMode::Islands),
            "1" | "spa" => Ok(ProjectRenderMode::Spa),
            "2" | "ssr" => Ok(ProjectRenderMode::Ssr),
            _ => Err("输入 0、1 或 2，也接受 islands / spa / ssr".to_owned()),
        },
    )?;
    style.done_line("渲染模式", render_mode_label(mode));
    Ok(mode)
}

fn prompt_database(style: &Style) -> Result<Option<ProjectDatabase>, String> {
    let database = prompt_menu(
        style,
        "数据库",
        "可选——启用 Toasty 依赖、DATABASE_URL 与迁移目录",
        &[
            ("0", "无", "不带数据库依赖，发布二进制最小"),
            ("1", "SQLite", "本地文件数据库，零配置"),
            ("2", "PostgreSQL", "启用 pgsql 驱动 feature"),
            ("3", "MySQL", "启用 MySQL / MariaDB 驱动 feature"),
            ("4", "全部", "同时编译三种驱动"),
        ],
        "0",
        |value| match value {
            "0" | "none" | "no" | "n" => Ok(None),
            "1" | "sqlite" => Ok(Some(ProjectDatabase::Sqlite)),
            "2" | "pgsql" | "postgres" | "postgresql" => Ok(Some(ProjectDatabase::Pgsql)),
            "3" | "mysql" | "mariadb" => Ok(Some(ProjectDatabase::Mysql)),
            "4" | "all" => Ok(Some(ProjectDatabase::All)),
            _ => Err("输入 0–4，也接受 none / sqlite / pgsql / mysql / all".to_owned()),
        },
    )?;
    style.done_line("数据库", database.map_or("无", database_label));
    Ok(database)
}

/// 官方 Feature 多选：逗号分隔编号或名称，回车全跳过。
fn prompt_features(style: &Style) -> Result<Vec<ProjectFeature>, String> {
    style.step_header("官方 Feature", "可多选，逗号分隔编号，回车全部跳过");
    for (index, feature) in ProjectFeature::ALL.iter().enumerate() {
        println!("  {}) {} — {}", index + 1, feature.key(), feature.summary());
    }
    loop {
        print!("  选择 []: ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let value = prompt_input(None)?;
        if value.is_empty() {
            style.done_line("Feature", "无");
            return Ok(Vec::new());
        }
        match parse_feature_selection(&value) {
            Ok(features) => {
                let labels = features
                    .iter()
                    .map(|feature| feature.key())
                    .collect::<Vec<_>>()
                    .join(", ");
                style.done_line("Feature", &labels);
                return Ok(features);
            }
            Err(error) => eprintln!("  ! {error}"),
        }
    }
}

/// Accept `1,3`, `captcha,notify`, or a mix; deduplicated and ordered.
fn parse_feature_selection(value: &str) -> Result<Vec<ProjectFeature>, String> {
    let mut features = Vec::new();
    for part in value.split([',', '，']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let feature = match part {
            "1" => ProjectFeature::Captcha,
            "2" => ProjectFeature::Pay,
            "3" => ProjectFeature::Notify,
            other => other.parse::<ProjectFeature>().map_err(|_| {
                format!("无法识别 `{other}`；输入编号 1–3 或名称 captcha / pay / notify")
            })?,
        };
        if !features.contains(&feature) {
            features.push(feature);
        }
    }
    features.sort_unstable();
    Ok(features)
}

fn prompt_yes_no(
    style: &Style,
    title: &str,
    choices: &[(&str, &str, &str)],
    default_key: &str,
) -> Result<bool, String> {
    let selected = prompt_menu(
        style,
        title,
        "",
        choices,
        default_key,
        |value| match value {
            "0" | "n" | "no" => Ok(false),
            "1" | "y" | "yes" => Ok(true),
            _ => Err("输入 0 或 1，也接受 y / n".to_owned()),
        },
    )?;
    let label = choices
        .iter()
        .find(|(key, _, _)| (*key == "1") == selected)
        .map_or("", |(_, label, _)| *label);
    style.done_line(title, label);
    Ok(selected)
}

/// Print a numbered menu and read until the choice parses.
///
/// Enter keeps `default_key`. The default marker is derived only from that key.
fn prompt_menu<T>(
    style: &Style,
    title: &str,
    blurb: &str,
    choices: &[(&str, &str, &str)],
    default_key: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    style.step_header(title, blurb);
    for (key, label, help) in choices {
        let mark = if *key == default_key {
            style.dim("  ← 默认")
        } else {
            String::new()
        };
        // Avoid `{label:<N}`: CJK width ≠ Rust char width and misaligns the menu.
        println!("  {key}) {label} — {help}{mark}");
    }
    loop {
        print!("  选择 [{default_key}]: ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let value = prompt_input(Some(default_key))?;
        // Also accept the visible label text (for example Islands, None, or No).
        let value = choices
            .iter()
            .find(|(_, label, _)| label.eq_ignore_ascii_case(&value) || *label == value)
            .map_or(value, |(key, _, _)| (*key).to_owned());
        match parse(&value) {
            Ok(parsed) => return Ok(parsed),
            Err(error) => {
                eprintln!("  ! {error}");
            }
        }
    }
}

fn prompt_input(default: Option<&str>) -> Result<String, String> {
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let trimmed = line.trim();
    Ok(if trimmed.is_empty() {
        default.unwrap_or_default().to_owned()
    } else {
        // ASCII lowercased; CJK tokens (是/否/无/全部) stay unchanged.
        trimmed.to_ascii_lowercase()
    })
}

fn make_auth(arguments: Vec<String>) -> Result<(), String> {
    let mut options = GenerateOptions::default();
    for flag in arguments {
        match flag.as_str() {
            "--force" | "-f" => options.force = true,
            _ => return Err(format!("unknown auth option `{flag}`")),
        }
    }
    let generator = current_generator()?;
    let written = generator.auth(options).map_err(|error| error.to_string())?;
    finish_generation(&generator, &written)
}

fn make_controller(arguments: Vec<String>) -> Result<(), String> {
    let (name, flags) = required_name(arguments)?;
    let mut options = ControllerOptions::default();
    for flag in flags {
        match flag.as_str() {
            "--resource" | "-r" => {
                options.resource = true;
                options.route = true;
            }
            "--route" => options.route = true,
            "--force" | "-f" => options.force = true,
            _ => return Err(format!("unknown controller option `{flag}`")),
        }
    }
    let generator = current_generator()?;
    let written = generator
        .controller(&name, options)
        .map_err(|error| error.to_string())?;
    finish_generation(&generator, &written)
}

fn make_model(arguments: Vec<String>) -> Result<(), String> {
    let (name, flags) = required_name(arguments)?;
    let mut options = ModelOptions::default();
    for flag in flags {
        match flag.as_str() {
            "--all" | "-a" => options.all = true,
            "--migration" | "-m" => options.migration = true,
            "--controller" | "-c" => options.controller = true,
            "--resource" | "-r" => {
                options.controller = true;
                options.resource_controller = true;
            }
            "--request" => options.request = true,
            "--api-resource" => options.api_resource = true,
            "--page" => options.page = true,
            "--force" | "-f" => options.force = true,
            _ => return Err(format!("unknown model option `{flag}`")),
        }
    }
    let generator = current_generator()?;
    let written = generator
        .model(&name, options)
        .map_err(|error| error.to_string())?;
    finish_generation(&generator, &written)
}

fn make_simple<F>(arguments: Vec<String>, generate: F) -> Result<(), String>
where
    F: FnOnce(
        &ProjectGenerator,
        &str,
        GenerateOptions,
    ) -> Result<Vec<PathBuf>, phoenix_cli::ScaffoldError>,
{
    let (name, flags) = required_name(arguments)?;
    let mut options = GenerateOptions::default();
    for flag in flags {
        match flag.as_str() {
            "--force" | "-f" => options.force = true,
            _ => return Err(format!("unknown generator option `{flag}`")),
        }
    }
    let generator = current_generator()?;
    let written = generate(&generator, &name, options).map_err(|error| error.to_string())?;
    finish_generation(&generator, &written)
}

fn required_name(mut arguments: Vec<String>) -> Result<(String, Vec<String>), String> {
    if arguments.is_empty() {
        return Err("this command requires a name".to_owned());
    }
    let name = arguments.remove(0);
    if name.starts_with('-') {
        return Err("the generated artifact name must come before options".to_owned());
    }
    Ok((name, arguments))
}

fn current_generator() -> Result<ProjectGenerator, String> {
    let current = env::current_dir().map_err(|error| error.to_string())?;
    ProjectGenerator::discover(current).map_err(|error| error.to_string())
}

fn print_written(generator: &ProjectGenerator, paths: &[PathBuf]) {
    for path in paths {
        println!(
            "WROTE {}",
            path.strip_prefix(generator.root())
                .unwrap_or(path)
                .display()
        );
    }
}

fn finish_generation(generator: &ProjectGenerator, paths: &[PathBuf]) -> Result<(), String> {
    print_written(generator, paths);
    if generator
        .refresh_types()
        .map_err(|error| error.to_string())?
    {
        println!("REFRESHED views/generated contracts and routes");
    } else {
        println!("Type files will refresh automatically after npm install or px dev");
    }
    Ok(())
}

fn require_empty(arguments: &[String]) -> Result<(), String> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", arguments.join(" ")))
    }
}
