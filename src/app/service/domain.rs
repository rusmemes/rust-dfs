use libp2p::gossipsub::SubscriptionError;
use libp2p::identity::DecodingError;
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::cbor;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, TransportError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkResponse {
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum P2pNetworkError {
    #[error("Failed to get directory of the keypair files: {0}")]
    FailedToGetKeypairFileDir(PathBuf),
    #[error("I/O error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    KeypairDecoding(#[from] DecodingError),
    #[error("Libp2p noise error: {0}")]
    Libp2pNoise(#[from] noise::Error),
    #[error("Libp2p swarm builder error: {0}")]
    Libp2pSwarmBuilder(String),
    #[error("Parsing libp2p multiaddress error: {0}")]
    Libp2pMultiAddrParse(#[from] libp2p::multiaddr::Error),
    #[error("Libp2p transport error: {0}")]
    Libp2pTransport(#[from] TransportError<std::io::Error>),
    #[error("Libp2p gossipsub subscription error: {0}")]
    Libp2pGossipsubSubscription(#[from] SubscriptionError),
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
}
