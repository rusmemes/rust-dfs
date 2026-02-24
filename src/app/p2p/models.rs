use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFile {
    pub total_chunks: usize,
    pub merkle_root: [u8; 32],
}
