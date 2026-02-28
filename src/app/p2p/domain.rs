use crate::app::file_processing::processing::FileProcessingResult;
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::cbor;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, gossipsub, identify, kad, mdns, ping, relay};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileRequest {
    pub file_id: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum FileResponse {
    Success(Vec<u8>),
    NotFound,
    Error(String),
}

pub enum P2pCommand {
    RequestMetadata {
        request: FileRequest,
        result: oneshot::Sender<Option<FileProcessingResult>>,
    },
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
    pub file_download: cbor::Behaviour<FileRequest, FileResponse>,
    pub metadata_download: cbor::Behaviour<FileRequest, FileResponse>,
}
