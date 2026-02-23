use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileProcessingError {
    #[error("File cannot be accessed: {0}")]
    FileAccess(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Merkle tree cannot be created: {0}")]
    MerkleTreeCreation(String),
}
