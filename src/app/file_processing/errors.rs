use thiserror::Error;
use tokio::task::JoinError;

#[derive(Debug, Error)]
pub enum FileProcessingError {
    #[error("File cannot be accessed: {0}")]
    FileAccess(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Merkle tree cannot be created: {0}")]
    MerkleTreeCreation(String),
    #[error("Metadata file cannot be created: {0}")]
    Cbor(#[from] serde_cbor::Error),
    #[error("Task join error: {0}")]
    TaskJoin(#[from] JoinError),
}
