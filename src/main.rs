use crate::app::cli::{Cli, Command};
use crate::app::server::Server;
use clap::Parser;
use tokio::select;
use tokio_util::sync::CancellationToken;

mod app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Command::Start => start(cli).await?,
    }

    Ok(())
}

async fn start(cli: Cli) -> anyhow::Result<()> {
    let cancellation_token = CancellationToken::new();

    let server = Server::new(cli, cancellation_token.clone());

    server.start().await?;

    select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = cancellation_token.cancelled() => {}
    }

    server.stop().await?;
    Ok(())
}
