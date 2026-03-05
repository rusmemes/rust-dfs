use crate::app::file_processing::processing::{restore_original_file, FileMetadata};
use crate::app::file_store::domain::{
    PendingDownloadRecord, PublishedFileKey, PublishedFileRecord,
};
use crate::app::file_store::FileStore;
use crate::app::p2p::domain::{DownloadFileChunk, FileChunkRequest, FileResponse, P2pCommand};
use crate::app::utils::{verify_chunk, METADATA_FILE_NAME};
use libp2p::futures::StreamExt;
use log::{debug, error, info};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::task::JoinSet;

const LOG_TARGET: &str = "app::file_download_service";

pub struct FileDownloadService<S: FileStore> {
    active_downloads_semaphore: Arc<Semaphore>,
    store: S,
    active_downloads: Arc<Mutex<HashSet<PublishedFileKey>>>,
    commands_tx: mpsc::Sender<P2pCommand>,
}

impl<S: FileStore> FileDownloadService<S> {

    pub fn new(store: S, max_active_downloads: u16, commands_tx: mpsc::Sender<P2pCommand>) -> Self {
        Self {
            store,
            commands_tx,
            active_downloads: Arc::new(Mutex::new(HashSet::new())),
            active_downloads_semaphore: Arc::new(Semaphore::new(max_active_downloads as usize)),
        }
    }

    pub async fn work_on_pending_downloads(&mut self, tx: mpsc::Sender<P2pCommand>) {
        if self.active_downloads_semaphore.available_permits() == 0 {
            info!(target: LOG_TARGET, "no download permits available");
            return;
        }
        let mut stream = self.store.stream_pending_downloads();
        while let Some(result) = stream.next().await {
            if self.active_downloads_semaphore.available_permits() == 0 {
                info!(target: LOG_TARGET, "no download permits available");
                return;
            }
            match result {
                Ok(mut pending_download_record) => {
                    let active_downloads = self.active_downloads.clone();

                    if active_downloads
                        .lock()
                        .await
                        .insert(pending_download_record.key.clone())
                    {
                        let semaphore = self.active_downloads_semaphore.clone();
                        let store = self.store.clone();
                        let command_tx = self.commands_tx.clone();
                        let sender = tx.clone();
                        tokio::spawn(async move {
                            match Self::download(
                                &mut pending_download_record,
                                semaphore,
                                store.clone(),
                                command_tx,
                            )
                            .await
                            {
                                Ok(completed) => {
                                    if completed {
                                        if let Err(error) = Self::process_completed(
                                            store,
                                            &mut pending_download_record,
                                            sender,
                                        )
                                        .await
                                        {
                                            error!(target: LOG_TARGET, "failed to complete download: {}", error);
                                        }
                                    }
                                }
                                Err(error) => {
                                    error!(target: LOG_TARGET, "Failed to start download: {}", error);
                                }
                            }
                            active_downloads
                                .lock()
                                .await
                                .remove(&pending_download_record.key);
                        });
                    }
                }
                Err(error) => {
                    error!(target: LOG_TARGET, "Error reading from stream: {}", error);
                }
            }
        }
    }

    async fn download<FS: FileStore>(
        pending_download_record: &mut PendingDownloadRecord,
        semaphore: Arc<Semaphore>,
        store: FS,
        commands_tx: mpsc::Sender<P2pCommand>,
    ) -> anyhow::Result<bool> {
        let metadata: FileMetadata =
            tokio::fs::read(pending_download_record.chunks_dir.join(METADATA_FILE_NAME))
                .await?
                .try_into()?;

        let mut join_set = JoinSet::new();
        for chunk_id in 0..metadata.merkle_proofs.len() {
            if pending_download_record
                .downloaded_chunks
                .contains(&chunk_id)
            {
                continue;
            }

            let download_file_chunk = DownloadFileChunk {
                file_id: pending_download_record.key.clone().into(),
                chunk_id,
                target_dir: pending_download_record.chunks_dir.clone(),
            };

            let local_semaphore = semaphore.clone();
            let sender = commands_tx.clone();

            join_set.spawn(tokio::spawn(async move {
                let permit = local_semaphore.acquire().await?;
                debug!("permit acquired {:?}", permit);
                let result = Self::download_file_chunk(download_file_chunk, sender).await;
                debug!("permit released {:?}", permit);
                result
            }));
        }

        drop(commands_tx);

        let mut first_error = None;
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(result) => match result {
                    Ok(result) => match result {
                        Ok(Some(chunk_id)) => {
                            info!(target: LOG_TARGET, "{}: chunk {} has been downloaded", &metadata.original_file_name, chunk_id);
                            if pending_download_record.downloaded_chunks.insert(chunk_id) {
                                if let Err(error) = store
                                    .put_pending_download(pending_download_record.clone())
                                    .await
                                {
                                    error!("failed to update pending download: {}", error);
                                }
                            }
                        }
                        Ok(None) => {
                            debug!(target: LOG_TARGET, "no file chunk present");
                        }
                        Err(error) => {
                            error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &metadata.original_file_name, error);
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        }
                    },
                    Err(error) => {
                        error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &metadata.original_file_name, error);
                        if first_error.is_none() {
                            first_error = Some(error.into());
                        }
                    }
                },
                Err(error) => {
                    error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &metadata.original_file_name, error);
                    if first_error.is_none() {
                        first_error = Some(error.into());
                    }
                }
            }
        }

        first_error.map(Err).unwrap_or(Ok(
            pending_download_record.downloaded_chunks.len() == metadata.merkle_proofs.len()
        ))
    }

    async fn download_file_chunk(
        download_file_chunk: DownloadFileChunk,
        commands_tx: mpsc::Sender<P2pCommand>,
    ) -> anyhow::Result<Option<usize>> {
        let (tx, rx) = oneshot::channel();

        commands_tx
            .send(P2pCommand::RequestFileChunk {
                request: FileChunkRequest {
                    file_id: download_file_chunk.file_id,
                    chunk_id: download_file_chunk.chunk_id,
                },
                result: tx,
            })
            .await?;

        Ok(match rx.await? {
            Some(FileResponse::Success(bytes)) => {
                Self::verify_and_save_file_chunk(&download_file_chunk, bytes).await?;
                if let Err(error) = commands_tx
                    .send(P2pCommand::ProvideFileChunk(FileChunkRequest {
                        file_id: download_file_chunk.file_id,
                        chunk_id: download_file_chunk.chunk_id,
                    }))
                    .await
                {
                    error!(target: LOG_TARGET, "failed to send file chunk providing command: {}", error);
                    None
                } else {
                    Some(download_file_chunk.chunk_id)
                }
            }
            Some(FileResponse::Error(err)) => {
                error!(target: LOG_TARGET, "failed to download chunk: {}", err);
                None
            }
            _ => None,
        })
    }

    async fn verify_and_save_file_chunk(
        download_file_chunk: &DownloadFileChunk,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let metadata: FileMetadata =
            tokio::fs::read(download_file_chunk.target_dir.join(METADATA_FILE_NAME))
                .await?
                .try_into()?;

        let is_correct = verify_chunk(
            &bytes,
            download_file_chunk.chunk_id,
            metadata
                .merkle_proofs
                .get(download_file_chunk.chunk_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("chunk {} not found", download_file_chunk.chunk_id)
                })?,
            metadata.merkle_root,
            metadata.total_chunks,
        );

        if is_correct {
            tokio::fs::write(
                download_file_chunk.target_dir.join(format!(
                    "{}.{}",
                    download_file_chunk.chunk_id, metadata.chunk_file_extension
                )),
                &bytes,
            )
            .await?;

            Ok(())
        } else {
            Err(anyhow::anyhow!("downloaded chunk is not correct"))
        }
    }

    async fn process_completed<FS: FileStore>(
        store: FS,
        pending_download_record: &mut PendingDownloadRecord,
        sender: mpsc::Sender<P2pCommand>,
    ) -> anyhow::Result<()> {
        let metadata = restore_original_file(&pending_download_record).await?;
        store
            .put_published_file(PublishedFileRecord {
                key: pending_download_record.key.clone(),
                original_file_name: pending_download_record.original_file_name.clone(),
                chunks_dir: pending_download_record.chunks_dir.clone(),
                public: metadata.public,
            })
            .await?;
        store
            .delete_pending_download(pending_download_record.key.clone())
            .await?;
        sender.send(P2pCommand::ProvideMetadata(metadata)).await?;
        Ok(())
    }
}
