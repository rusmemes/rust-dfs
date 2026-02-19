use crate::app::{P2pNetworkError, P2pService, P2pServiceConfig};
use async_trait::async_trait;
use log::info;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

const LOG_TARGET: &str = "app::server";

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
    #[error("P2P Network error: {0}")]
    P2pNetwork(#[from] P2pNetworkError),
}

pub type ServerResult<T> = Result<T, ServerError>;

pub struct Server {
    cancellation_token: CancellationToken,
    subtasks: Arc<Mutex<Vec<JoinHandle<Result<(), ServerError>>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&self, cancellation_token: CancellationToken) -> Result<(), ServerError>;
}

impl Server {
    pub fn new() -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
            subtasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn start(&self) -> ServerResult<()> {
        let p2p_service = P2pService::new(
            P2pServiceConfig::builder()
                .with_keypair_file("./keys.keypair")
                .build(),
        );
        self.spawn_task(p2p_service).await?;
        Ok(())
    }

    async fn spawn_task<S: Service>(&self, service: S) -> ServerResult<()> {
        let cancellation_token = self.cancellation_token.clone();
        let mut subtasks = self.subtasks.lock().await;

        subtasks.push(tokio::spawn(async move {
            service.start(cancellation_token).await
        }));

        Ok(())
    }

    pub async fn stop(&self) -> ServerResult<()> {
        info!(target: LOG_TARGET, "Shutting down...");
        self.cancellation_token.cancel();
        let mut subtasks = self.subtasks.lock().await;
        for handle in subtasks.iter_mut() {
            handle.await??
        }
        Ok(())
    }
}
