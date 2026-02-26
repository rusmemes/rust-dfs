use crate::app::file_store::errors::FileStoreError;
use crate::app::grpc::errors::GrpcServerError;
use crate::app::p2p::errors::P2pNetworkError;
use thiserror::Error;
use tokio::task::JoinError;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
    #[error("P2P Network error: {0}")]
    P2pNetwork(#[from] P2pNetworkError),
    #[error("GRPC Server error: {0}")]
    GrpcServer(#[from] GrpcServerError),
    #[error("File store error: {0}")]
    FileStore(#[from] FileStoreError),
}
