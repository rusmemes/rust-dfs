use crate::app::file_processing::processing::FileMetadata;
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::cbor;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, gossipsub, identify, kad, mdns, ping, relay, PeerId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MetadataFileRequest {
    pub file_id: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: u64,
    pub chunk_id: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum FileResponse {
    Success(Vec<u8>),
    NotFound,
    Error(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileFound {
    pub file_id: u64,
    pub file_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileSearchResult {
    pub session_id: String,
    pub file_found: FileFound,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileSearchRequest {
    pub session_id: String,
    pub search_value: String,
}

impl TryInto<FileSearchRequest> for Vec<u8> {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<FileSearchRequest, Self::Error> {
        serde_cbor::from_slice(&self)
    }
}

impl TryInto<Vec<u8>> for FileSearchRequest {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

pub enum P2pCommand {
    RequestMetadata {
        request: MetadataFileRequest,
        result: oneshot::Sender<Option<FileMetadata>>,
    },
    RequestFileChunk {
        request: FileChunkRequest,
        result: oneshot::Sender<Option<FileResponse>>,
    },
    ProvideMetadata(FileMetadata),
    ProvideFileChunk(FileChunkRequest),

    FileSearch(FileSearchRequest, mpsc::Sender<FileFound>),
    FileSearchResult(PeerId, FileSearchResult),
    FileSearchAbort(String)
}

#[derive(NetworkBehaviour)]
pub struct P2pNetworkBehaviour {
    pub ping: ping::Behaviour,
    pub identify: identify::Behaviour,
    pub mdns: mdns::Behaviour<mdns::tokio::Tokio>,
    pub kademlia: kad::Behaviour<MemoryStore>,
    pub gossipsub: gossipsub::Behaviour,
    pub relay_server: relay::Behaviour,
    pub relay_client: relay::client::Behaviour,
    pub dcutr: dcutr::Behaviour,
    pub file_download: cbor::Behaviour<FileChunkRequest, FileResponse>,
    pub metadata_download: cbor::Behaviour<MetadataFileRequest, FileResponse>,
    pub file_search_results: cbor::Behaviour<FileSearchResult, ()>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishedFile {
    pub total_chunks: usize,
    pub merkle_root: [u8; 32],
}

#[derive(Debug)]
pub struct DownloadFileChunk {
    pub file_id: u64,
    pub chunk_id: usize,
    pub target_dir: PathBuf,
}
