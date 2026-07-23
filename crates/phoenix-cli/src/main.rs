use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command, ExitCode},
};

use phoenix_cli::{
    ControllerOptions, DependencySource, DevConfig, DevSupervisor, GenerateOptions, ModelOptions,
    NewProjectOptions, ProjectGenerator, create_project, release_build, release_install,
    release_rollback, release_status,
};

const HELP: &str = r"Phoenix-rs application CLI (px)

Install: cargo install px-cli
         cargo install --git https://github.com/MageGojo/Phoenix-rs px-cli
         cargo install --path crates/phoenix-cli

Usage:
  px new <project> [--framework-path <path>] [--no-install] [--no-git]
  px dev
  px migrate
  px status
  px rollback [--step <count>]
  px fresh [--seed]
  px seed
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
  px migrate
  px rollback --step 2
  px fresh --seed
  px make:model Post --all
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
        "migrate" => database_command("migrate", &no_options(&arguments)?),
        "status" => database_command("status", &no_options(&arguments)?),
        "rollback" => database_command("rollback", &rollback_options(&arguments)?),
        "fresh" => database_command("fresh", &fresh_options(&arguments)?),
        "seed" => database_command("seed", &no_options(&arguments)?),
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
    println!("  backend:     cargo run -- serve");
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

fn new_project(arguments: Vec<String>) -> Result<(), String> {
    let (target, flags) = required_name(arguments)?;
    let mut options = NewProjectOptions::new(&target);
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--no-install" => options.install_dependencies = false,
            "--no-git" => options.initialize_git = false,
            "--framework-path" => {
                index += 1;
                let path = flags.get(index).ok_or("--framework-path requires a path")?;
                options.dependencies = DependencySource::Local(PathBuf::from(path));
            }
            flag => return Err(format!("unknown new-project option `{flag}`")),
        }
        index += 1;
    }
    create_project(&options).map_err(|error| error.to_string())?;
    println!(
        "Created Phoenix application at {}",
        options.target.display()
    );
    println!("Next: cd {} && px dev", options.target.display());
    Ok(())
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
