use crate::app::file_store::rocksdb::RocksDbStoreError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileStoreError {
    #[error("RocksDB store error: {0}")]
    RocksDb(#[from] RocksDbStoreError),
    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
}
