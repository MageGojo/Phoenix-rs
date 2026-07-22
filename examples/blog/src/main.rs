use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::var("APP_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let server = phoenix_blog_example::application()?.bind(&address).await?;

    println!(
        "Phoenix blog example listening on http://{}",
        server.local_addr()
    );
    server
        .run_with_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
