use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use phoenix_cli::{
    ControllerOptions, DependencySource, DevConfig, DevSupervisor, GenerateOptions, ModelOptions,
    NewProjectOptions, ProjectGenerator, create_project,
};

const HELP: &str = r"Phoenix application CLI

Usage:
  phoenix new <project> [--framework-path <path>] [--no-install] [--no-git]
  phoenix dev
  phoenix make:controller <name> [--resource] [--route] [--force]
  phoenix make:model <name> [--all] [--migration] [--controller] [--resource]
                            [--request] [--api-resource] [--page] [--force]
  phoenix make:migration <name> [--force]
  phoenix make:request <name> [--force]
  phoenix make:resource <name> [--force]
  phoenix make:middleware <name> [--force]
  phoenix make:page <path> [--force]
  phoenix make:island <name> [--force]
  phoenix list

Examples:
  phoenix new my-app
  phoenix make:model Post --all
  phoenix make:controller Admin/ReportController --resource
  phoenix make:page posts/index
";

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Phoenix command failed: {error}");
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
        "dev" => {
            require_empty(&arguments)?;
            DevSupervisor::new(DevConfig::default())
                .run()
                .await
                .map_err(|error| error.to_string())
        }
        "new" => new_project(arguments),
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
        "list" => {
            require_empty(&arguments)?;
            print!("{HELP}");
            Ok(())
        }
        _ => Err(format!("unknown command `{command}`\n\n{HELP}")),
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
    println!("Next: cd {} && phoenix dev", options.target.display());
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
        println!("Type files will refresh automatically after npm install or phoenix dev");
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
