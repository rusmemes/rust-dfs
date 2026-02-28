use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::errors::FileStoreError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod errors;
pub mod rocksdb;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublishedFileRecord {
    pub key: PublishedFileKey,
    pub original_file_name: String,
    pub target_dir: PathBuf,
    pub public: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublishedFileKey(pub [u8; 8]);

impl From<PublishedFileKey> for u64 {
    fn from(key: PublishedFileKey) -> u64 {
        u64::from_be_bytes(key.0)
    }
}

impl From<[u8; 8]> for PublishedFileKey {
    fn from(key: [u8; 8]) -> PublishedFileKey {
        PublishedFileKey(key)
    }
}

impl From<u64> for PublishedFileKey {
    fn from(key: u64) -> PublishedFileKey {
        PublishedFileKey(key.to_be_bytes())
    }
}

impl From<FileProcessingResult> for PublishedFileRecord {
    fn from(result: FileProcessingResult) -> Self {
        Self {
            key: result.key().into(),
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

impl TryFrom<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(&bytes)
    }
}

pub trait FileStore: Store + Send + Sync + Clone + 'static {}
impl<T> FileStore for T where T: Store + Send + Sync + Clone + 'static {}

#[async_trait]
pub trait Store {
    async fn get_published_file(
        &self,
        key: PublishedFileKey,
    ) -> Result<Option<PublishedFileRecord>, FileStoreError>;
    async fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), FileStoreError>;
    async fn persist_file_processing_result(
        &self,
        record: FileProcessingResult,
    ) -> Result<(), FileStoreError>;

    async fn get_next_file_processing_result(&self)
    -> Result<FileProcessingResult, FileStoreError>;
    async fn delete_file_processing_result(
        &self,
        file_processing_result_key: [u8; 8],
    ) -> Result<(), FileStoreError>;
}
