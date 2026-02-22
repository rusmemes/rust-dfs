pub mod publish {
    tonic::include_proto!("publish");
}

use crate::app::errors::ServerError;
use crate::app::grpc::errors::GrpcServerError;
use crate::app::grpc::server::publish::publish_file_response;
use crate::app::server::Service;
use async_trait::async_trait;
use log::info;
use publish::publish_service_server::{PublishService as Publish, PublishServiceServer};
use publish::{PublishFileRequest, PublishFileResponse};
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

const LOG_TARGET: &str = "app::grpc::server";

pub struct PublishService {}

impl PublishService {
    pub fn new() -> Self {
        Self {}
    }
}

#[tonic::async_trait]
impl Publish for PublishService {
    async fn publish_file(
        &self,
        request: Request<PublishFileRequest>,
    ) -> Result<Response<PublishFileResponse>, Status> {
        info!(target: LOG_TARGET, "We got a new publish file request: {:?}", request);
        Ok(Response::new(PublishFileResponse {
            result: Some(publish_file_response::Result::Ok(())),
        }))
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
            .add_service(PublishServiceServer::new(PublishService::new()))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
