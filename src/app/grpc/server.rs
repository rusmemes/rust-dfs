use crate::app::errors::ServerError;
use crate::app::file_processing::processing::{split_file, FileSplitResult};
use crate::app::grpc::errors::GrpcServerError;
use crate::app::grpc::publish::publish_service_server::PublishService as Publish;
use crate::app::grpc::publish::publish_service_server::PublishServiceServer;
use crate::app::grpc::publish::PublishFileRequest;
use crate::app::grpc::publish::PublishFileResponse;
use crate::app::server::Service;
use async_trait::async_trait;
use log::info;
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher, MerkleProof};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct PublishService;

#[tonic::async_trait]
impl Publish for PublishService {
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<Response<PublishFileResponse>, Status> {
        let request = request.into_inner();

        let _ = split_file(&request.file_path)
            .await
            .map_err(|e| Status::internal(format!("failed to split file: {}", e)))?;

        // todo

        Ok(Response::new(PublishFileResponse { ok: Some(()) }))
    }
}

pub struct GrpcService {
    port: u16,
}

impl GrpcService {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

#[async_trait]
impl Service for GrpcService {
    async fn start(&self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;

        info!(target: LOG_TARGET, "Grpc Server is starting at {}", grpc_address);

        Server::builder()
            .add_service(PublishServiceServer::new(PublishService))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
