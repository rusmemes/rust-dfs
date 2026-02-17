use std::sync::Arc;
use thiserror::Error;
use tokio::select;
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
    subtasks: Arc<Mutex<JoinSet<()>>>,
}

impl Server {

    pub fn new() -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
            subtasks: Arc::new(Mutex::new(JoinSet::new())),
        }
    }

    pub async fn start(&self) -> ServerResult<()> {
        let cancellation_token = self.cancellation_token.clone();
        let mut subtasks = self.subtasks.lock().await;

        subtasks.spawn(async move {
            loop {
                select! {
                    _ = cancellation_token.cancelled() => {
                        println!("Stopping server...");
                        break
                    },
                }
            }
        });

        Ok(())
    }

    pub async fn stop(&self) -> ServerResult<()> {
        self.cancellation_token.cancel();
        let mut subtasks = self.subtasks.lock().await;
        while let Some(res) = subtasks.join_next().await {
            res?
        }
        Ok(())
    }
}
