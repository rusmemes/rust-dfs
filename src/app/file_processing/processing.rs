use crate::app::file_processing::errors::FileProcessingError;
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher, MerkleTree};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

#[derive(Debug)]
pub struct FileSplitResult<'a> {
    pub original_file_path: &'a str,
    pub total_chunks: usize,
    pub target_dir: PathBuf,
    pub merkle_root: [u8; 32],
    pub merkle_proofs: Vec<Vec<u8>>,
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

    let merkle_tree_leaves = split(file, &pieces_dir).await?;
    let merkle_tree = MerkleTree::<Sha256>::from_leaves(&merkle_tree_leaves);
    let merkle_root = merkle_tree.root().ok_or_else(|| {
        FileProcessingError::MerkleTreeCreation("cannot get merkle root".to_owned())
    })?;

    let merkle_proofs = (0usize..merkle_tree_leaves.len())
        .map(|index| merkle_tree.proof(&[index]).to_bytes())
        .collect::<Vec<_>>();

    Ok(FileSplitResult {
        original_file_path,
        total_chunks: merkle_tree_leaves.len(),
        target_dir: pieces_dir,
        merkle_root,
        merkle_proofs,
    })
}

async fn split(file: File, pieces_dir: &PathBuf) -> Result<Vec<[u8; 32]>, FileProcessingError> {
    const CHUNK_SIZE: usize = 1024 * 1024; // 1mb
    let mut heap_buffer = vec![0u8; CHUNK_SIZE];

    let mut reader = tokio::io::BufReader::new(file);
    let mut merkle_tree_leaves: Vec<[u8; 32]> = Vec::new();
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

        let path = pieces_dir.join(format!("{}.chunk", merkle_tree_leaves.len()));
        let bytes = &heap_buffer[..filled];
        tokio::fs::write(&path, bytes).await?;
        merkle_tree_leaves.push(Sha256::hash(bytes))
    }

    Ok(merkle_tree_leaves)
}
