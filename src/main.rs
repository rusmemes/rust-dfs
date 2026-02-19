use app::Server;
mod app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let server = Server::new();

    server.start().await?;

    tokio::signal::ctrl_c().await?;

    server.stop().await?;

    Ok(())
}
