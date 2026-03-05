use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::domain::{
    PendingDownloadRecord, PublishedFileKey, PublishedFileRecord,
};
use crate::app::file_store::errors::FileStoreError;
use async_trait::async_trait;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

pub mod domain;
pub mod errors;
pub mod rocksdb;

pub trait FileStore: Store + Send + Sync + Clone + 'static {}
impl<T> FileStore for T where T: Store + Send + Sync + Clone + 'static {}

#[async_trait]
pub trait Store {
    async fn put_published_file(&self, record: PublishedFileRecord) -> Result<(), FileStoreError>;
    async fn get_published_file(
        &self,
        key: PublishedFileKey,
    ) -> Result<Option<PublishedFileRecord>, FileStoreError>;
    fn stream_published_files(&self)
    -> ReceiverStream<Result<PublishedFileRecord, FileStoreError>>;
    async fn put_pending_download(
        &self,
        record: PendingDownloadRecord,
    ) -> Result<(), FileStoreError>;
    async fn get_pending_download(
        &self,
        key: PublishedFileKey,
    ) -> Result<Option<PendingDownloadRecord>, FileStoreError>;
    async fn delete_pending_download(&self, key: PublishedFileKey) -> Result<(), FileStoreError>;
    fn stream_pending_downloads(
        &self,
    ) -> ReceiverStream<Result<PendingDownloadRecord, FileStoreError>>;
    async fn put_file_processing_result(
        &self,
        record: FileProcessingResult,
    ) -> Result<(), FileStoreError>;
    async fn delete_file_processing_result(
        &self,
        file_processing_result_key: [u8; 8],
    ) -> Result<(), FileStoreError>;
    async fn get_next_file_processing_result(&self)
    -> Result<FileProcessingResult, FileStoreError>;
}
