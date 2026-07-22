use std::process::ExitCode;

use phoenix_cli::{DevConfig, DevSupervisor};

#[tokio::main]
async fn main() -> ExitCode {
    let mut arguments = std::env::args_os();
    let _binary = arguments.next();
    match arguments.next().as_deref() {
        Some(command) if command == "dev" && arguments.next().is_none() => {
            match DevSupervisor::new(DevConfig::default()).run().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("Phoenix dev failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("Usage: phoenix dev");
            ExitCode::FAILURE
        }
    }
}
