use crate::app::file_processing::errors::FileProcessingError;
use crate::app::file_processing::processing::FileProcessingResult;
use futures_util::future::select_all;
use std::io::{BufWriter, ErrorKind};
use std::path::PathBuf;
use tokio::task::JoinHandle;

pub async fn ensure_dir_exists_or_create(path_buf: &PathBuf) -> Result<(), std::io::Error> {
    if tokio::fs::try_exists(&path_buf).await? {
        if !tokio::fs::metadata(&path_buf).await?.is_dir() {
            return Err(std::io::Error::new(
                ErrorKind::NotADirectory,
                format!("{:?} is not a directory", path_buf),
            ));
        }
    } else {
        tokio::fs::create_dir_all(&path_buf).await?;
    }

    Ok(())
}

pub async fn save_metadata(
    result: FileProcessingResult,
) -> Result<FileProcessingResult, FileProcessingError> {
    tokio::task::spawn_blocking(move || save_blocking(result)).await?
}

pub const METADATA_FILE_NAME: &str = "files.cbor";

fn save_blocking(
    result: FileProcessingResult,
) -> Result<FileProcessingResult, FileProcessingError> {
    let file = std::fs::File::create(result.target_dir.join(METADATA_FILE_NAME))?;
    let writer = BufWriter::new(file);
    serde_cbor::to_writer(writer, &result)?;
    Ok(result)
}

pub async fn on_each_join<T, R, F>(mut handles: Vec<JoinHandle<T>>, callback: F) -> Vec<R>
where
    F: Fn(T) -> R,
{
    let mut result = Vec::with_capacity(handles.len());
    while !handles.is_empty() {
        let (res, _idx, remaining) = select_all(handles).await;

        if let Ok(res) = res {
            result.push(callback(res));
        }

        handles = remaining;
    }

    result
}
