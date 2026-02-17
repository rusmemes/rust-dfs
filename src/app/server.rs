use crate::app::P2pService;
use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
}

pub type ServerResult<T> = Result<T, Error>;

pub struct Server {
    cancellation_token: CancellationToken,
    subtasks: Arc<Mutex<JoinSet<Result<(), Error>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&self, cancellation_token: CancellationToken) -> Result<(), Error>;
}

impl Server {
    pub fn new() -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
            subtasks: Arc::new(Mutex::new(JoinSet::new())),
        }
    }

    pub async fn start(&self) -> ServerResult<()> {
        let p2p_service = P2pService::new();
        self.spawn_task(p2p_service).await?;
        Ok(())
    }

    async fn spawn_task<S: Service>(&self, service: S) -> ServerResult<()> {
        let cancellation_token = self.cancellation_token.clone();
        let mut subtasks = self.subtasks.lock().await;

        subtasks.spawn(async move { service.start(cancellation_token).await });

        Ok(())
    }

    pub async fn stop(&self) -> ServerResult<()> {
        self.cancellation_token.cancel();
        let mut subtasks = self.subtasks.lock().await;
        while let Some(res) = subtasks.join_next().await {
            res??
        }
        Ok(())
    }
}
