use crate::app::{Error, Service};
use async_trait::async_trait;
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::cbor;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{dcutr, gossipsub, identify, kad, mdns, ping, relay};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkRequest {
    pub file_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileChunkResponse {
    pub data: Vec<u8>,
}

#[derive(NetworkBehaviour)]
pub struct P2pNetworkBehaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    mdns: mdns::Behaviour<mdns::tokio::Tokio>,
    kademlia: kad::Behaviour<MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    relay_server: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    file_download: cbor::Behaviour<FileChunkRequest, FileChunkResponse>,
}

pub struct P2pService {}

impl P2pService {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Service for P2pService {
    async fn start(&self, cancellation_token: CancellationToken) -> Result<(), Error> {
        todo!()
    }
}
