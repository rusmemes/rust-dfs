use crate::app::errors::ServerError;
use crate::app::file_processing::processing::process_file;
use crate::app::file_store::{FileStore, PublishedFileKey};
use crate::app::grpc::dfs_grpc::dfs_server::Dfs;
use crate::app::grpc::dfs_grpc::dfs_server::DfsServer;
use crate::app::grpc::dfs_grpc::PublishFileResponse;
use crate::app::grpc::dfs_grpc::{DownloadFileRequest, DownloadFileResponse, PublishFileRequest};
use crate::app::grpc::errors::GrpcServerError;
use crate::app::server::Service;
use async_trait::async_trait;
use log::info;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct DfsService<S: FileStore> {
    store: S,
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
            .persist_file_processing_result(file_split_result)
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
        let _key: PublishedFileKey = request.file_id.into();
        // todo
        Ok(Response::new(DownloadFileResponse { ok: Some(()) }))
    }
}

pub struct GrpcService<S: FileStore> {
    port: u16,
    store: S,
}

impl<S> GrpcService<S>
where
    S: FileStore,
{
    pub fn new(port: u16, store: S) -> Self {
        Self { port, store }
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
            }))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
