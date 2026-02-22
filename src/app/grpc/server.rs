pub mod publish {
    tonic::include_proto!("publish");
}

use crate::app::errors::ServerError;
use crate::app::grpc::errors::GrpcServerError;
use crate::app::server::Service;
use async_trait::async_trait;
use log::info;
use publish::publish_service_server::{PublishService as Publish, PublishServiceServer};
use publish::{PublishFileRequest, PublishFileResponse};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
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
        let request = request.into_inner();

        let metadata = tokio::fs::metadata(request.file_path.clone())
            .await
            .map_err(|_| Status::internal("cannot get file metadata"))?;

        if !metadata.is_file() {
            return Err(Status::internal("not a file"));
        }

        let file = File::open(request.file_path.clone())
            .await
            .map_err(|_| Status::internal("cannot open file"))?;

        let file_path = PathBuf::from(request.file_path);

        let containing_dir = file_path
            .parent()
            .ok_or_else(|| Status::internal("cannot get parent dir"))?;

        let file_name = file_path
            .file_name()
            .ok_or_else(|| Status::internal("cannot get file name"))?;

        let pieces_dir = Path::new(containing_dir).join(format!(
            "{}_chunks",
            file_name.to_string_lossy().replace(".", "_")
        ));

        tokio::fs::create_dir_all(&pieces_dir).await?;

        let mut buffer = [0; 1024 * 1024]; // 1mb
        let mut reader = tokio::io::BufReader::new(file);
        let mut chunk_number = 0;
        loop {
            let size_read = reader
                .read(&mut buffer)
                .await
                .map_err(|_| Status::internal("cannot read the file"))?;

            if size_read == 0 {
                break;
            }

            let path = pieces_dir.join(format!("{}.chunk", chunk_number));
            tokio::fs::write(&path, &buffer[0..size_read]).await?;
            chunk_number += 1;
        }

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
            .add_service(PublishServiceServer::new(PublishService::new()))
            .serve_with_shutdown(grpc_address, cancellation_token.cancelled())
            .await
            .map_err(|error| GrpcServerError::Transport(error))?;

        info!(target: LOG_TARGET, "Grpc Server stopped");

        Ok(())
    }
}
