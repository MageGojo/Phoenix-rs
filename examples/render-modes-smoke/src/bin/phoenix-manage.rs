#[cfg(feature = "database")]
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

    let config = render_modes_smoke::config::load()?;
    let mut database = render_modes_smoke::database(&config).await?;
    if command == "seed" {
        require_no_options(options)?;
        render_modes_smoke::seeders::run(&mut database).await?;
        println!("Seeders completed.");
        return Ok(());
    }

    let mut runner = MigrationRunner::new(
        &mut database,
        render_modes_smoke::migrations::all(),
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
                render_modes_smoke::seeders::run(&mut database).await?;
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
