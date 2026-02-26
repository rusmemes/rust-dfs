use libp2p::kad::store::MemoryStore;
use libp2p::request_response::cbor;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, gossipsub, identify, kad, mdns, ping, relay};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkResponse {
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MetadataDownloadRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MetadataDownloadResponse {
    pub data: Vec<u8>,
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
    pub file_download: cbor::Behaviour<FileChunkRequest, FileChunkResponse>,
    pub metadata_download: cbor::Behaviour<MetadataDownloadRequest, MetadataDownloadResponse>,
}
