use phoenix::prelude::{CommandResult, Console, LogFormat, Logging};

use render_modes_smoke::commands;

#[tokio::main]
async fn main() -> CommandResult {
    Console::new(env!("CARGO_PKG_NAME"))
        .about("Phoenix application")
        .serve(|_ctx| async move {
            let config = render_modes_smoke::config::load()?;
            let address = config.address().to_owned();
            let public_url = config.public_url().to_owned();
            let production = config.environment().is_production();
            let _logging = Logging::new()
                .format(if production {
                    LogFormat::Json
                } else {
                    LogFormat::Compact
                })
                .ansi(!production)
                .init()?;
            let server = render_modes_smoke::application(config)
                .await?
                .bind(&address)
                .await?;
            println!(
                "Phoenix application ready at {public_url} (listening on {})",
                server.local_addr()
            );
            server
                .run_with_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
            Ok(())
        })
        .commands(commands::registry())
        .run()
        .await
}
