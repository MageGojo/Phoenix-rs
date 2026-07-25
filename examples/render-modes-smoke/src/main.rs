use phoenix::prelude::{CommandResult, Console, LogFormat, Logging};

use render_modes_smoke::{commands, features};

#[tokio::main]
async fn main() -> CommandResult {
    let plugins = features::plugins()?;
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
            let application = render_modes_smoke::application(config).await?;

            #[cfg(feature = "tls")]
            if let (Ok(tls_addr), Ok(cert), Ok(key)) = (
                std::env::var("APP_TLS_ADDR"),
                std::env::var("APP_TLS_CERT"),
                std::env::var("APP_TLS_KEY"),
            ) {
                let tls = phoenix::prelude::TlsConfig::from_files(&cert, &key)?;
                let server = application.bind_tls(&tls_addr, tls).await?;
                println!(
                    "Phoenix application ready at https://{} (TLS listening on {})",
                    tls_addr,
                    server.local_addr()
                );
                server
                    .run_with_shutdown(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await?;
                return Ok(());
            }

            let server = application.bind(&address).await?;
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
        .commands(
            commands::registry()
                .into_iter()
                .chain(plugins.into_commands()),
        )
        .run()
        .await
}
