use crate::app::file_processing::errors::FileProcessingError;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

#[derive(Debug)]
pub struct FileSplitResult<'a> {
    pub original_file_path: &'a str,
    pub total_chunks: u32,
    pub target_dir: PathBuf,
}

pub async fn split_file(file_path: &str) -> Result<FileSplitResult<'_>, FileProcessingError> {
    let metadata = tokio::fs::metadata(file_path)
        .await
        .map_err(|_| FileProcessingError::FileAccess("File metadata not found".to_owned()))?;

    if !metadata.is_file() {
        return Err(FileProcessingError::FileAccess("Not a file".to_owned()));
    }

    let file = File::open(file_path)
        .await
        .map_err(|_| FileProcessingError::FileAccess("cannot open file".to_owned()))?;

    let original_file_path = file_path;
    let file_path = PathBuf::from(original_file_path);

    let containing_dir = file_path
        .parent()
        .ok_or_else(|| FileProcessingError::FileAccess("cannot get parent dir".to_owned()))?;

    let file_name = file_path
        .file_name()
        .ok_or_else(|| FileProcessingError::FileAccess("cannot get file name".to_owned()))?;

    let pieces_dir = Path::new(containing_dir).join(format!(
        "{}_chunks",
        file_name.to_string_lossy().replace(".", "_")
    ));

    tokio::fs::create_dir_all(&pieces_dir).await?;

    let total_chunks = split(file, &pieces_dir).await?;

    Ok(FileSplitResult {
        original_file_path,
        total_chunks,
        target_dir: pieces_dir,
    })
}

async fn split(file: File, pieces_dir: &PathBuf) -> Result<u32, FileProcessingError> {
    const CHUNK_SIZE: usize = 1024 * 1024; // 1mb
    let mut heap_buffer = vec![0u8; CHUNK_SIZE];

    let mut reader = tokio::io::BufReader::new(file);
    let mut chunk_number: u32 = 0;

    loop {
        let mut filled = 0;

        while filled < CHUNK_SIZE {
            let size_read = reader
                .read(&mut heap_buffer[filled..])
                .await
                .map_err(|_| FileProcessingError::FileAccess("cannot read the file".to_owned()))?;

            if size_read == 0 {
                break;
            }

            filled += size_read;
        }

        if filled == 0 {
            break;
        }

        let path = pieces_dir.join(format!("{}.chunk", chunk_number));
        tokio::fs::write(&path, &heap_buffer[..filled]).await?;
        chunk_number += 1;
    }

    Ok(chunk_number)
}
