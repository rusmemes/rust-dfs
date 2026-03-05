use crate::app::errors::ServerError;
use crate::app::file_processing::processing::{process_file, FileMetadata};
use crate::app::file_store::domain::{PendingDownloadRecord, PublishedFileKey};
use crate::app::file_store::FileStore;
use crate::app::grpc::dfs_grpc::dfs_server::Dfs;
use crate::app::grpc::dfs_grpc::dfs_server::DfsServer;
use crate::app::grpc::dfs_grpc::PublishFileResponse;
use crate::app::grpc::dfs_grpc::{DownloadFileRequest, DownloadFileResponse, PublishFileRequest};
use crate::app::grpc::errors::GrpcServerError;
use crate::app::p2p::domain::{MetadataFileRequest, P2pCommand};
use crate::app::server::Service;
use crate::app::utils::{ensure_dir_exists_or_create, save_metadata};
use async_trait::async_trait;
use log::info;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Code, Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct DfsService<S: FileStore> {
    store: S,
    command_sender: mpsc::Sender<P2pCommand>,
}

#[tonic::async_trait]
impl<S> Dfs for DfsService<S>
where
    S: FileStore,
{
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<Response<PublishFileResponse>, Status> {
        let request = request.into_inner();

        let file_split_result = process_file(&request.file_path, request.public)
            .await
            .map_err(|e| Status::internal(format!("failed to split file: {}", e)))?;

        let key: PublishedFileKey = file_split_result.key().into();

        self.store
            .put_file_metadata(file_split_result)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(PublishFileResponse {
            file_id: key.into(),
        }))
    }

    async fn download_file(
        &self,
        request: Request<DownloadFileRequest>,
    ) -> Result<Response<DownloadFileResponse>, Status> {
        let request = request.into_inner();

        let download_path = PathBuf::from(request.download_path);

        ensure_dir_exists_or_create(&download_path)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let (tx, rx) = oneshot::channel();

        self.command_sender
            .send(P2pCommand::RequestMetadata {
                request: MetadataFileRequest {
                    file_id: request.file_id,
                },
                result: tx,
            })
            .await
            .map_err(|_| {
                Status::new(Code::Internal, "failed to send P2pCommand::RequestMetadata")
            })?;

        let metadata = rx
            .await
            .map_err(|_| Status::new(Code::Internal, "failed to receive response"))?;

        let metadata =
            metadata.ok_or_else(|| Status::new(Code::Internal, "missing file"))?;

        let file_path = download_path.join(&metadata.original_file_name);

        if tokio::fs::try_exists(&file_path).await?
            || self
                .store
                .pending_download_exists(metadata.key().into())
                .await
                .map_err(|e| Status::internal(e.to_string()))?
        {
            return Err(Status::already_exists(file_path.to_string_lossy()));
        }

        let chunks_dir = download_path.join(format!(
            "{}_chunks",
            metadata.get_chunks_dir_name()
        ));

        ensure_dir_exists_or_create(&chunks_dir)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let metadata = FileMetadata {
            chunks_dir,
            ..metadata
        };

        let pending_download = PendingDownloadRecord::new(
            metadata.key().into(),
            metadata.original_file_name.clone(),
            metadata.chunks_dir.clone(),
        );

        save_metadata(metadata)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        self.store
            .put_pending_download(pending_download)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(DownloadFileResponse { ok: Some(()) }))
    }
}

pub struct GrpcService<S: FileStore> {
    port: u16,
    store: S,
    command_sender: mpsc::Sender<P2pCommand>,
}

impl<S> GrpcService<S>
where
    S: FileStore,
{
    pub fn new(port: u16, store: S, command_sender: mpsc::Sender<P2pCommand>) -> Self {
        Self {
            port,
            store,
            command_sender,
        }
    }
}

#[async_trait]
impl<S> Service for GrpcService<S>
where
    S: FileStore,
{
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;

        info!(target: LOG_TARGET, "Grpc Server is starting at {}", grpc_address);

        Server::builder()
            .add_service(DfsServer::new(DfsService {
                store: self.store.clone(),
                command_sender: self.command_sender.clone(),
            }))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
