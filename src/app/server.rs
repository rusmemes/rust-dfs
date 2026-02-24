use crate::app::errors::ServerError;
use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::grpc::server::GrpcService;
use crate::app::p2p::config::P2pServiceConfig;
use crate::app::p2p::service::P2pService;
use async_trait::async_trait;
use log::{error, info};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const LOG_TARGET: &str = "app::server";

pub type ServerResult<T> = Result<T, ServerError>;

pub struct Server {
    cancellation_token: CancellationToken,
    subtasks: Arc<Mutex<Vec<JoinHandle<Result<(), ServerError>>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError>;
}

impl Server {
    pub fn new(cancellation_token: CancellationToken) -> Self {
        Self {
            cancellation_token,
            subtasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn start(&self) -> ServerResult<()> {
        let (sender, receiver) = mpsc::channel::<FileProcessingResult>(1);

        let p2p_service = P2pService::new(
            P2pServiceConfig::builder()
                .with_keypair_file("./keys.keypair")
                .build(),
            receiver,
        );
        self.spawn_task(p2p_service).await?;

        let grpc_service = GrpcService::new(9999, sender);
        self.spawn_task(grpc_service).await?;

        Ok(())
    }

    async fn spawn_task<S: Service>(&self, mut service: S) -> ServerResult<()> {
        let cancellation_token = self.cancellation_token.clone();
        let mut subtasks = self.subtasks.lock().await;

        subtasks.push(tokio::spawn(async move {
            let result = service.start(cancellation_token.child_token()).await;
            if let Err(error) = result {
                error!(target: LOG_TARGET, "{}", error);
                cancellation_token.cancel();
            }
            Ok(())
        }));

        Ok(())
    }

    pub async fn stop(&self) -> ServerResult<()> {
        info!(target: LOG_TARGET, "Shutting down...");
        self.cancellation_token.cancel();
        let mut subtasks = self.subtasks.lock().await;
        for handle in subtasks.iter_mut() {
            match handle.await {
                Ok(result) => {
                    if let Err(e) = result {
                        error!(target: LOG_TARGET, "{}", e);
                    }
                }
                Err(error) => {
                    error!(target: LOG_TARGET, "{}", error);
                }
            }
        }
        Ok(())
    }
}
