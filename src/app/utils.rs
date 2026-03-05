use crate::app::file_processing::errors::FileProcessingError;
use crate::app::file_processing::processing::FileMetadata;
use rand::RngExt;
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher as MerkleHasher, MerkleProof};
use std::collections::HashSet;
use std::io::{BufWriter, ErrorKind};
use std::path::PathBuf;

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
    result: FileMetadata,
) -> Result<FileMetadata, FileProcessingError> {
    tokio::task::spawn_blocking(move || save_blocking(result)).await?
}

pub const METADATA_FILE_NAME: &str = ".metadata";

fn save_blocking(
    result: FileMetadata,
) -> Result<FileMetadata, FileProcessingError> {
    let file = std::fs::File::create(result.chunks_dir.join(METADATA_FILE_NAME))?;
    let writer = BufWriter::new(file);
    serde_cbor::to_writer(writer, &result)?;
    Ok(result)
}

pub fn verify_chunk(
    chunk: &[u8],
    index: usize,
    proof_bytes: &[u8],
    root: [u8; 32],
    total_leaves: usize,
) -> bool {
    let leaf = Sha256::hash(chunk);
    let proof = MerkleProof::<Sha256>::from_bytes(proof_bytes).unwrap();
    proof.verify(root, &[index], &[leaf], total_leaves)
}

pub fn choose_random<T>(set: &HashSet<T>) -> Option<&T> {
    if set.len() == 1 {
        set.iter().next()
    } else if !set.is_empty() {
        let mut rng = rand::rng();
        let i = rng.random_range(0..set.len());
        set.iter().nth(i)
    } else {
        None
    }
}
