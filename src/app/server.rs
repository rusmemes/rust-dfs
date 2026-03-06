use crate::app::cli::Cli;
use crate::app::errors::ServerError;
use crate::app::file_store::rocksdb::RocksDBStore;
use crate::app::grpc::server::GrpcService;
use crate::app::p2p::config::P2pServiceConfig;
use crate::app::p2p::service::P2pService;
use async_trait::async_trait;
use log::{error, info};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const LOG_TARGET: &str = "app::server";

pub type ServerResult<T> = Result<T, ServerError>;

pub struct Server {
    cli: Cli,
    cancellation_token: CancellationToken,
    subtasks: Arc<Mutex<Vec<JoinHandle<Result<(), ServerError>>>>>,
}

#[async_trait]
pub trait Service: Send + Sync + 'static {
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError>;
}

impl Server {
    pub fn new(cli: Cli, cancellation_token: CancellationToken) -> Self {
        Self {
            cli,
            cancellation_token,
            subtasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn start(&self) -> ServerResult<()> {
        init_base_dir(&self.cli.base_path).await?;

        let store = RocksDBStore::new(self.cli.base_path.join("file_store")).await?;

        // p2p commands channel
        let (tx, rx) = mpsc::channel(100);

        let mut builder = P2pServiceConfig::builder()
            .with_keypair_file(self.cli.base_path.join("key.keypair"))
            .with_bootstrap_peers(self.cli.bootstrap_peers.clone());

        if let Some(topic) = &self.cli.file_search_topic {
            builder = builder.with_file_search_topic(topic.clone());
        }
        
        let p2p_service = P2pService::new(
            builder.build(),
            store.clone(),
            rx,
            tx.clone(),
            self.cli.max_active_downloads,
        );
        self.spawn_task(p2p_service).await?;

        let grpc_service = GrpcService::new(self.cli.grpc_port, store, tx);
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

async fn init_base_dir(base_dir: &PathBuf) -> ServerResult<()> {
    if tokio::fs::try_exists(base_dir).await? {
        let result = tokio::fs::metadata(base_dir).await?;
        if !result.is_dir() {
            Err(ServerError::Custom(format!(
                "{} is not a directory",
                base_dir.display()
            )))?;
        }
    } else {
        tokio::fs::create_dir_all(base_dir).await?;
    }

    Ok(())
}
