use app::Server;
mod app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = Server::new();

    server.start().await?;

    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");

    server.stop().await?;

    Ok(())
}
