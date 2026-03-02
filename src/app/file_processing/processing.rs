use crate::app::file_processing::errors::FileProcessingError;
use crate::app::utils::save_metadata;
use rs_merkle::algorithms::Sha256;
use rs_merkle::{Hasher as MerkleHasher, MerkleTree};
use rs_sha256::Sha256Hasher;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub const FILE_CHUNK_EXTENSION: &str = "chunk";

#[derive(Debug, Serialize, Deserialize)]
pub struct FileProcessingResult {
    pub original_file_name: String,
    pub total_chunks: usize,
    pub target_dir: PathBuf,
    pub merkle_root: [u8; 32],
    pub merkle_proofs: Vec<Vec<u8>>,
    pub chunk_file_extension: String,
    pub public: bool,
}

impl FileProcessingResult {
    pub fn key(&self) -> [u8; 8] {
        let mut hasher = Sha256Hasher::default();
        self.hash(&mut hasher);
        hasher.finish().to_be_bytes()
    }
}

impl Hash for FileProcessingResult {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.original_file_name.hash(state);
        self.total_chunks.hash(state);
        self.merkle_root.hash(state);
        self.public.hash(state);
    }
}

impl TryFrom<Vec<u8>> for FileProcessingResult {
    type Error = serde_cbor::Error;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(&value)
    }
}

impl TryFrom<FileProcessingResult> for Vec<u8> {
    type Error = serde_cbor::Error;

    fn try_from(value: FileProcessingResult) -> Result<Self, Self::Error> {
        serde_cbor::to_vec(&value)
    }
}

pub async fn process_file(
    file_path: &str,
    public: bool,
) -> Result<FileProcessingResult, FileProcessingError> {
    let metadata = tokio::fs::metadata(file_path).await?;

    if !metadata.is_file() {
        return Err(FileProcessingError::FileAccess("Not a file".to_owned()));
    }

    let file = File::open(file_path).await?;

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

    let file_split_result = FileProcessingResult {
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

async fn split(file: File, pieces_dir: &PathBuf) -> Result<Vec<[u8; 32]>, FileProcessingError> {
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
