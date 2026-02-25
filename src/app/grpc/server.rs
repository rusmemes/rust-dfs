use crate::app::errors::ServerError;
use crate::app::file_processing::processing::process_file;
use crate::app::file_store;
use crate::app::grpc::errors::GrpcServerError;
use crate::app::grpc::publish::publish_service_server::PublishService as Publish;
use crate::app::grpc::publish::publish_service_server::PublishServiceServer;
use crate::app::grpc::publish::PublishFileRequest;
use crate::app::grpc::publish::PublishFileResponse;
use crate::app::server::Service;
use async_trait::async_trait;
use file_store::Store;
use log::info;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct PublishService<S: Store + Send + Sync + 'static + Clone> {
    store: S,
}

#[tonic::async_trait]
impl<S> Publish for PublishService<S>
where
    S: Store + Send + Sync + 'static + Clone,
{
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<Response<PublishFileResponse>, Status> {
        let request = request.into_inner();

        let file_split_result = process_file(&request.file_path, request.public)
            .await
            .map_err(|e| Status::internal(format!("failed to split file: {}", e)))?;

        self.store
            .persist_file_processing_result(file_split_result)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(PublishFileResponse { ok: Some(()) }))
    }
}

pub struct GrpcService<S: Store + Send + Sync + 'static + Clone> {
    port: u16,
    store: S,
}

impl<S> GrpcService<S>
where
    S: Store + Send + Sync + 'static + Clone,
{
    pub fn new(port: u16, store: S) -> Self {
        Self { port, store }
    }
}

#[async_trait]
impl<S> Service for GrpcService<S>
where
    S: Store + Send + Sync + 'static + Clone,
{
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let grpc_address = format!("127.0.0.1:{}", self.port)
            .as_str()
            .parse()
            .map_err(|error| GrpcServerError::AddressParse(error))?;

        info!(target: LOG_TARGET, "Grpc Server is starting at {}", grpc_address);

        Server::builder()
            .add_service(PublishServiceServer::new(PublishService {
                store: self.store.clone(),
            }))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
