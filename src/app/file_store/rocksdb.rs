use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::errors::FileStoreError;
use crate::app::file_store::{PublishedFileKey, PublishedFileRecord, Store};
use async_trait::async_trait;
use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;

const PUBLISHED_FILES_COLUMN_FAMILY_NAME: &str = "published_files";
const JOBS_COLUMN_FAMILY_NAME: &str = "jobs";

#[derive(Clone)]
pub struct RocksDBStore {
    db: Arc<rocksdb::DB>,
}

#[derive(Error, Debug)]
pub enum RocksDbStoreError {
    #[error("RocksDB error: {0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("Column family does not exist: {0}")]
    CfMissing(String),
    #[error("Cbor error: {0}")]
    CBor(#[from] serde_cbor::Error),
    #[error("Join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl RocksDBStore {
    pub async fn new<T: Into<PathBuf>>(path: T) -> Result<Self, FileStoreError> {
        let path = path.into();

        tokio::task::spawn_blocking(move || {
            let mut opts = Options::default();
            opts.create_if_missing(true);
            opts.create_missing_column_families(true);

            let published_files =
                ColumnFamilyDescriptor::new(PUBLISHED_FILES_COLUMN_FAMILY_NAME, opts.clone());
            let jobs = ColumnFamilyDescriptor::new(JOBS_COLUMN_FAMILY_NAME, opts.clone());

            let db = rocksdb::DB::open_cf_descriptors(&opts, path, vec![published_files, jobs])
                .map_err(RocksDbStoreError::RocksDb)?;

            Ok::<_, FileStoreError>(RocksDBStore { db: Arc::new(db) })
        })
        .await?
    }
}

#[async_trait]
impl Store for RocksDBStore {
    async fn published_file_exists(&self, key: PublishedFileKey) -> Result<bool, FileStoreError> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let cf = db
                .cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
                .ok_or_else(|| {
                    RocksDbStoreError::CfMissing(PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_owned())
                })?;

            Ok(db
                .get_pinned_cf(cf, key.0)
                .map_err(RocksDbStoreError::RocksDb)?
                .is_some())
        })
        .await?
    }

    async fn add_published_file(&self, record: PublishedFileRecord) -> Result<(), FileStoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let cf = db
                .cf_handle(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
                .ok_or_else(|| {
                    RocksDbStoreError::CfMissing(PUBLISHED_FILES_COLUMN_FAMILY_NAME.to_owned())
                })?;

            let key = record.key.clone();
            let value: Vec<u8> = record.try_into().map_err(|e| RocksDbStoreError::CBor(e))?;

            db.put_cf(cf, key.0, value)
                .map_err(|e| RocksDbStoreError::RocksDb(e))?;

            Ok(())
        })
        .await?
    }

    async fn persist_file_processing_result(
        &self,
        record: FileProcessingResult,
    ) -> Result<(), FileStoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let cf = db
                .cf_handle(JOBS_COLUMN_FAMILY_NAME)
                .ok_or_else(|| RocksDbStoreError::CfMissing(JOBS_COLUMN_FAMILY_NAME.to_owned()))?;

            let key = record.key();
            let value: Vec<u8> = record.try_into().map_err(|e| RocksDbStoreError::CBor(e))?;

            db.put_cf(cf, key, value)
                .map_err(|e| RocksDbStoreError::RocksDb(e))?;

            Ok(())
        })
        .await?
    }

    async fn get_next_file_processing_result(
        &self,
    ) -> Result<FileProcessingResult, FileStoreError> {
        loop {
            let db = self.db.clone();

            let result: Result<Option<FileProcessingResult>, FileStoreError> =
                tokio::task::spawn_blocking(move || {
                    let cf = db.cf_handle(JOBS_COLUMN_FAMILY_NAME).ok_or_else(|| {
                        RocksDbStoreError::CfMissing(JOBS_COLUMN_FAMILY_NAME.to_owned())
                    })?;

                    match db.iterator_cf(cf, IteratorMode::Start).next() {
                        Some(Ok((_, value))) => {
                            let result = value
                                .to_vec()
                                .try_into()
                                .map_err(|e| RocksDbStoreError::CBor(e))?;
                            Ok(Some(result))
                        }
                        Some(Err(e)) => Err(RocksDbStoreError::RocksDb(e).into()),
                        None => Ok(None),
                    }
                })
                .await?;

            match result {
                Ok(None) => {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Ok(Some(result)) => return Ok(result),
                Err(error) => return Err(error),
            }
        }
    }

    async fn delete_file_processing_result(
        &self,
        file_processing_result_key: [u8; 8],
    ) -> Result<(), FileStoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let cf = db
                .cf_handle(JOBS_COLUMN_FAMILY_NAME)
                .ok_or_else(|| RocksDbStoreError::CfMissing(JOBS_COLUMN_FAMILY_NAME.to_owned()))?;

            db.delete_cf(cf, file_processing_result_key)
                .map_err(|e| RocksDbStoreError::RocksDb(e))?;

            Ok(())
        })
        .await?
    }
}
