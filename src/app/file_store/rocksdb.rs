use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::domain::{Iterable, PendingDownloadRecord, Persistable};
use crate::app::file_store::errors::FileStoreError;
use crate::app::file_store::{PublishedFileKey, PublishedFileRecord, Store};
use async_trait::async_trait;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, IteratorMode, Options, DB};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

const PUBLISHED_FILES_COLUMN_FAMILY_NAME: &str = "published_files";
const PENDING_DOWNLOADS_COLUMN_FAMILY_NAME: &str = "pending_downloads";
const JOBS_COLUMN_FAMILY_NAME: &str = "jobs";

#[derive(Clone)]
pub struct RocksDBStore {
    db: Arc<DB>,
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
            let pending_downloads =
                ColumnFamilyDescriptor::new(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME, opts.clone());

            let db = DB::open_cf_descriptors(
                &opts,
                path,
                vec![published_files, jobs, pending_downloads],
            )
            .map_err(RocksDbStoreError::RocksDb)?;

            Ok::<_, FileStoreError>(RocksDBStore { db: Arc::new(db) })
        })
        .await?
    }

    async fn put<R: Persistable>(&self, record: R, cf: &'static str) -> Result<(), FileStoreError> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let (key, value) = record.try_into().map_err(|e| RocksDbStoreError::CBor(e))?;

            db.put_cf(get_cf(&db, cf)?, key.0, value)
                .map_err(|e| RocksDbStoreError::RocksDb(e))?;

            Ok(())
        })
        .await?
    }

    fn stream_all<R: Iterable>(
        &self,
        cf: &'static str,
    ) -> ReceiverStream<Result<R, FileStoreError>> {
        let db = self.db.clone();
        let (tx, rx) = mpsc::channel(64);

        tokio::task::spawn_blocking(move || {
            let cf = match get_cf(&db, cf) {
                Ok(cf) => cf,
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
            };

            let iterator = db.iterator_cf(cf, IteratorMode::Start);

            for item in iterator {
                if tx.is_closed() {
                    break;
                }

                let result = match item {
                    Ok((_, value)) => value
                        .to_vec()
                        .try_into()
                        .map_err(|e| RocksDbStoreError::CBor(e).into()),
                    Err(e) => Err(RocksDbStoreError::RocksDb(e).into()),
                };

                if tx.blocking_send(result).is_err() {
                    break;
                }
            }
        });

        ReceiverStream::new(rx)
    }
}

fn get_cf<'a>(db: &'a Arc<DB>, x: &str) -> Result<&'a ColumnFamily, FileStoreError> {
    Ok(db
        .cf_handle(x)
        .ok_or_else(|| RocksDbStoreError::CfMissing(x.to_owned()))?)
}

#[async_trait]
impl Store for RocksDBStore {
    fn stream_published_files(
        &self,
    ) -> ReceiverStream<Result<PublishedFileRecord, FileStoreError>> {
        self.stream_all(PUBLISHED_FILES_COLUMN_FAMILY_NAME)
    }

    fn stream_pending_downloads(
        &self,
    ) -> ReceiverStream<Result<PendingDownloadRecord, FileStoreError>> {
        self.stream_all(PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)
    }

    async fn get_pending_download(
        &self,
        key: PublishedFileKey,
    ) -> Result<Option<PendingDownloadRecord>, FileStoreError> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let option = db
                .get_cf(get_cf(&db, PENDING_DOWNLOADS_COLUMN_FAMILY_NAME)?, key.0)
                .map_err(RocksDbStoreError::RocksDb)?;

            if let Some(record) = option {
                let result: Result<PendingDownloadRecord, serde_cbor::Error> = record.try_into();
                return Ok(result.map(Some).map_err(RocksDbStoreError::CBor)?);
            }

            Ok(None)
        })
        .await?
    }

    async fn get_published_file(
        &self,
        key: PublishedFileKey,
    ) -> Result<Option<PublishedFileRecord>, FileStoreError> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let option = db
                .get_cf(get_cf(&db, PUBLISHED_FILES_COLUMN_FAMILY_NAME)?, key.0)
                .map_err(RocksDbStoreError::RocksDb)?;

            if let Some(record) = option {
                let result: Result<PublishedFileRecord, serde_cbor::Error> = record.try_into();
                return Ok(result.map(Some).map_err(RocksDbStoreError::CBor)?);
            }

            Ok(None)
        })
        .await?
    }

    async fn put_published_file(&self, record: PublishedFileRecord) -> Result<(), FileStoreError> {
        self.put(record, PUBLISHED_FILES_COLUMN_FAMILY_NAME).await
    }

    async fn put_pending_download(
        &self,
        record: PendingDownloadRecord,
    ) -> Result<(), FileStoreError> {
        self.put(record, PENDING_DOWNLOADS_COLUMN_FAMILY_NAME).await
    }

    async fn put_file_processing_result(
        &self,
        record: FileProcessingResult,
    ) -> Result<(), FileStoreError> {
        self.put(record, JOBS_COLUMN_FAMILY_NAME).await
    }

    async fn get_next_file_processing_result(
        &self,
    ) -> Result<FileProcessingResult, FileStoreError> {
        loop {
            let db = self.db.clone();

            let result: Result<Option<FileProcessingResult>, FileStoreError> =
                tokio::task::spawn_blocking(move || {
                    match db
                        .iterator_cf(get_cf(&db, JOBS_COLUMN_FAMILY_NAME)?, IteratorMode::Start)
                        .next()
                    {
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
            db.delete_cf(
                get_cf(&db, JOBS_COLUMN_FAMILY_NAME)?,
                file_processing_result_key,
            )
            .map_err(|e| RocksDbStoreError::RocksDb(e))?;

            Ok(())
        })
        .await?
    }
}
