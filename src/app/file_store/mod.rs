use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::rocksdb::RocksDbStoreError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

pub mod rocksdb;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublishedFileRecord {
    pub original_file_name: String,
    pub target_dir: PathBuf,
    pub public: bool,
}

impl PublishedFileRecord {
    pub fn key(&self) -> Vec<u8> {
        self.original_file_name.as_bytes().to_vec()
    }
}

impl From<FileProcessingResult> for PublishedFileRecord {
    fn from(result: FileProcessingResult) -> Self {
        Self {
            original_file_name: result.original_file_name,
            target_dir: result.target_dir,
            public: result.public,
        }
    }
}

impl TryInto<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

#[derive(Error, Debug)]
pub enum FileStoreError {
    #[error("RocksDB store error: {0}")]
    RocksDb(#[from] RocksDbStoreError),
    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
}

#[async_trait]
pub trait Store {
    async fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), FileStoreError>;
    async fn persist_file_processing_result(&self, record: FileProcessingResult) -> Result<(), FileStoreError>;

    async fn get_next_file_processing_result(&self) -> Result<FileProcessingResult, FileStoreError>;
    async fn delete_file_processing_result(&self, file_processing_result_key: Vec<u8>) -> Result<(), FileStoreError>;
}
