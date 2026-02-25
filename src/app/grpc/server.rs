use crate::app::errors::ServerError;
use crate::app::file_processing::processing::{process_file, FileProcessingResult};
use crate::app::grpc::errors::GrpcServerError;
use crate::app::grpc::publish::publish_service_server::PublishService as Publish;
use crate::app::grpc::publish::publish_service_server::PublishServiceServer;
use crate::app::grpc::publish::PublishFileRequest;
use crate::app::grpc::publish::PublishFileResponse;
use crate::app::server::Service;
use async_trait::async_trait;
use log::info;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct PublishService {
    file_publish_sender: Sender<FileProcessingResult>,
}

#[tonic::async_trait]
impl Publish for PublishService {
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<Response<PublishFileResponse>, Status> {
        let request = request.into_inner();

        let file_split_result = process_file(&request.file_path, request.public)
            .await
            .map_err(|e| Status::internal(format!("failed to split file: {}", e)))?;

        if self
            .file_publish_sender
            .try_send(file_split_result)
            .is_err()
        {
            return Err(Status::internal("can't process the request now, try later"));
        }

        Ok(Response::new(PublishFileResponse { ok: Some(()) }))
    }
}

pub struct GrpcService {
    port: u16,
    file_publish_sender: Sender<FileProcessingResult>,
}

impl GrpcService {
    pub fn new(port: u16, file_publish_sender: Sender<FileProcessingResult>) -> Self {
        Self {
            port,
            file_publish_sender,
        }
    }
}

#[async_trait]
impl Service for GrpcService {
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;

        info!(target: LOG_TARGET, "Grpc Server is starting at {}", grpc_address);

        // TODO: do replay

        Server::builder()
            .add_service(PublishServiceServer::new(PublishService {
                file_publish_sender: self.file_publish_sender.clone(),
            }))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
