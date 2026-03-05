use crate::app::file_processing::errors::FileProcessingError;
use crate::app::file_store::domain::{PendingDownloadRecord, PublishedFileKey};
use crate::app::utils::{save_metadata, METADATA_FILE_NAME};
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher as MerkleHasher, MerkleTree};
use rs_sha256::Sha256Hasher;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const FILE_CHUNK_EXTENSION: &str = "chunk";

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub original_file_name: String,
    pub total_chunks: usize,
    pub target_dir: PathBuf,
    pub merkle_root: [u8; 32],
    pub merkle_proofs: Vec<Vec<u8>>,
    pub chunk_file_extension: String,
    pub public: bool,
}

impl FileMetadata {
    pub fn key(&self) -> [u8; 8] {
        let mut hasher = Sha256Hasher::default();
        self.hash(&mut hasher);
        hasher.finish().to_be_bytes()
    }
}

impl Hash for FileMetadata {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.original_file_name.hash(state);
        self.total_chunks.hash(state);
        self.merkle_root.hash(state);
        self.public.hash(state);
    }
}

impl TryInto<(PublishedFileKey, Vec<u8>)> for FileMetadata {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<(PublishedFileKey, Vec<u8>), Self::Error> {
        let key: PublishedFileKey = PublishedFileKey(self.key());
        TryFrom::try_from(self).map(|bytes| (key, bytes))
    }
}

impl TryFrom<Vec<u8>> for FileMetadata {
    type Error = serde_cbor::Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(&value)
    }
}

impl TryFrom<FileMetadata> for Vec<u8> {
    type Error = serde_cbor::Error;

    fn try_from(value: FileMetadata) -> Result<Self, Self::Error> {
        serde_cbor::to_vec(&value)
    }
}

pub async fn process_file(
    file_path: &str,
    public: bool,
) -> Result<FileMetadata, FileProcessingError> {
    let metadata = tokio::fs::metadata(file_path).await?;

    if !metadata.is_file() {
        return Err(FileProcessingError::FileAccess("Not a file".to_owned()));
    }

    let file = tokio::fs::File::open(file_path).await?;

    let original_file_path = file_path;
    let file_path = PathBuf::from(original_file_path);

    let containing_dir = file_path
        .parent()
        .ok_or_else(|| FileProcessingError::FileAccess("cannot get parent dir".to_owned()))?;

    let file_name = file_path
        .file_name()
        .ok_or_else(|| FileProcessingError::FileAccess("cannot get file name".to_owned()))?
        .to_string_lossy();

    let pieces_dir =
        Path::new(containing_dir).join(format!("{}_chunks", file_name.replace(".", "_")));

    tokio::fs::create_dir_all(&pieces_dir).await?;

    let merkle_tree_leaves = split(file, &pieces_dir).await?;
    let merkle_tree = MerkleTree::<Sha256>::from_leaves(&merkle_tree_leaves);
    let merkle_root = merkle_tree.root().ok_or_else(|| {
        FileProcessingError::MerkleTreeCreation("cannot get merkle root".to_owned())
    })?;

    let merkle_proofs = (0usize..merkle_tree_leaves.len())
        .map(|index| merkle_tree.proof(&[index]).to_bytes())
        .collect::<Vec<_>>();

    let file_split_result = FileMetadata {
        original_file_name: file_name.to_string(),
        total_chunks: merkle_tree_leaves.len(),
        target_dir: pieces_dir,
        merkle_root,
        merkle_proofs,
        chunk_file_extension: String::from(FILE_CHUNK_EXTENSION),
        public,
    };

    Ok(save_metadata(file_split_result).await?)
}

async fn split(
    file: tokio::fs::File,
    pieces_dir: &PathBuf,
) -> Result<Vec<[u8; 32]>, FileProcessingError> {
    const CHUNK_SIZE: usize = 1024 * 1024; // 1mb
    let mut heap_buffer = vec![0u8; CHUNK_SIZE];

    let mut reader = tokio::io::BufReader::new(file);
    let mut merkle_tree_leaves: Vec<[u8; 32]> = Vec::new();
    loop {
        let mut filled = 0;

        while filled < CHUNK_SIZE {
            let size_read = reader.read(&mut heap_buffer[filled..]).await?;

            if size_read == 0 {
                break;
            }

            filled += size_read;
        }

        if filled == 0 {
            break;
        }

        let path = pieces_dir.join(format!(
            "{}.{FILE_CHUNK_EXTENSION}",
            merkle_tree_leaves.len()
        ));
        let bytes = &heap_buffer[..filled];
        tokio::fs::write(&path, bytes).await?;
        merkle_tree_leaves.push(Sha256::hash(bytes))
    }

    Ok(merkle_tree_leaves)
}

pub async fn restore_original_file(
    pending_download_record: &PendingDownloadRecord,
) -> Result<FileMetadata, FileProcessingError> {
    let file_processing_result: FileMetadata = tokio::fs::read(
        pending_download_record
            .download_path
            .join(METADATA_FILE_NAME),
    )
    .await?
    .try_into()?;

    let parent_dir = file_processing_result.target_dir.parent().ok_or_else(|| {
        FileProcessingError::FileAccess(format!(
            "cannot get parent dir: {}",
            &file_processing_result.target_dir.display()
        ))
    })?;

    let file_path = parent_dir.join(&file_processing_result.original_file_name);

    if tokio::fs::try_exists(&file_path).await? {
        let metadata = tokio::fs::metadata(&file_path).await?;
        if !metadata.is_file() {
            return Err(FileProcessingError::FileAccess(format!(
                "already exists but not a file: {}",
                file_path.display()
            )));
        }
        return Ok(file_processing_result);
    }

    let file = tokio::fs::File::create(&file_path).await?;
    let mut buf_writer = tokio::io::BufWriter::new(file);
    for chunk in 0..file_processing_result.total_chunks {
        let chunk_bytes = tokio::fs::read(file_processing_result.target_dir.join(format!(
            "{}.{}",
            chunk, file_processing_result.chunk_file_extension
        )))
        .await?;
        buf_writer.write(&chunk_bytes).await?;
    }

    Ok(file_processing_result)
}
