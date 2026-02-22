use crate::app::server::Server;
use tokio::select;
use tokio_util::sync::CancellationToken;

mod app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cancellation_token = CancellationToken::new();

    let server = Server::new(cancellation_token.clone());

    server.start().await?;

    select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = cancellation_token.cancelled() => {}
    }

    server.stop().await?;

    Ok(())
}
