use phoenix::prelude::{LogFormat, Logging};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _logging = Logging::new().format(LogFormat::Compact).init()?;
    phoenix_multi_app_example::application()?
        .bind("127.0.0.1:3000")
        .await?
        .run()
        .await?;
    Ok(())
}
