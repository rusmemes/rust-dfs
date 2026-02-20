use crate::app::{P2pServiceConfig, ServerError, Service};
use async_trait::async_trait;
use libp2p::futures::StreamExt;
use libp2p::gossipsub::{IdentTopic, SubscriptionError};
use libp2p::identify::Event;
use libp2p::identity::{DecodingError, Keypair};
use libp2p::kad::store::MemoryStore;
use libp2p::kad::Mode;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::cbor;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, request_response, tcp,
    yamux, StreamProtocol, Swarm, TransportError,
};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::select;
use tokio_util::sync::CancellationToken;

const LOG_TARGET: &str = "app::p2p::service";

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

pub struct P2pService {
    config: P2pServiceConfig,
}

impl P2pService {
    pub fn new(config: P2pServiceConfig) -> Self {
        Self { config }
    }

    async fn keypair(&self) -> Result<Keypair, P2pNetworkError> {
        let exists = tokio::fs::try_exists(&self.config.keypair_file).await?;
        if exists {
            let data = tokio::fs::read(&self.config.keypair_file).await?;
            Ok(Keypair::from_protobuf_encoding(data.as_slice())?)
        } else {
            let dir = self.config.keypair_file.parent().ok_or(
                P2pNetworkError::FailedToGetKeypairFileDir(self.config.keypair_file.clone()),
            )?;
            tokio::fs::create_dir_all(dir).await?;

            let keypair = Keypair::generate_ed25519();
            let encoded = keypair.to_protobuf_encoding()?;
            tokio::fs::write(&self.config.keypair_file, &encoded).await?;
            Ok(keypair)
        }
    }

    async fn swarm(&self) -> Result<Swarm<P2pNetworkBehaviour>, P2pNetworkError> {
        let keypair = self.keypair().await?;
        Ok(libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key_pair, relay_client| {
                // kademlia config
                let mut kad_config = kad::Config::new(StreamProtocol::new("/dfs/1.0.0/kad"));
                kad_config.set_periodic_bootstrap_interval(Some(Duration::from_secs(30)));

                // gossipsub config
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .build()?;

                Ok(P2pNetworkBehaviour {
                    ping: ping::Behaviour::new(ping::Config::default()),
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/dfs/1.0.0".to_string(),
                        key_pair.public(),
                    )),
                    mdns: mdns::Behaviour::new(
                        mdns::Config::default(),
                        key_pair.public().to_peer_id(),
                    )?,
                    kademlia: kad::Behaviour::with_config(
                        key_pair.public().to_peer_id(),
                        MemoryStore::new(key_pair.public().to_peer_id()),
                        kad_config,
                    ),
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key_pair.clone()),
                        gossipsub_config,
                    )?,
                    relay_server: relay::Behaviour::new(
                        key_pair.public().to_peer_id(),
                        relay::Config::default(),
                    ),
                    relay_client,
                    dcutr: dcutr::Behaviour::new(key_pair.public().to_peer_id()),
                    file_download: cbor::Behaviour::new(
                        [(
                            StreamProtocol::new("/dfs/1.0.0/file-download"),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                })
            })
            .map_err(|e| P2pNetworkError::Libp2pSwarmBuilder(e.to_string()))?
            .with_swarm_config(|config| {
                config.with_idle_connection_timeout(Duration::from_secs(30))
            })
            .build())
    }

    fn log_debug<T: Debug>(event: &T) {
        debug!(target: LOG_TARGET, "{:?}", event);
    }

    fn handle_swarm_event(
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        event: SwarmEvent<P2pNetworkBehaviourEvent>,
    ) -> Result<(), ServerError> {
        match event {
            SwarmEvent::Behaviour(event) => match event {
                P2pNetworkBehaviourEvent::Identify(event) => Self::identify(swarm, event)?,
                P2pNetworkBehaviourEvent::Mdns(_) => {}
                P2pNetworkBehaviourEvent::Kademlia(_) => {}
                P2pNetworkBehaviourEvent::Gossipsub(_) => {}
                P2pNetworkBehaviourEvent::RelayServer(_) => {}
                P2pNetworkBehaviourEvent::RelayClient(_) => {}
                P2pNetworkBehaviourEvent::Dcutr(_) => {}
                P2pNetworkBehaviourEvent::FileDownload(_) => {}
                _ => Self::log_debug(&event),
            },
            SwarmEvent::NewListenAddr {
                listener_id: _listener_id,
                address,
            } => info!(target: LOG_TARGET, "Listening on {:?}", address),
            _ => Self::log_debug(&event),
        }

        Ok(())
    }

    fn identify(swarm: &mut Swarm<P2pNetworkBehaviour>, event: Event) -> Result<(), ServerError> {
        match event {
            Event::Received {
                connection_id: _connection_id,
                peer_id,
                info,
            } => {
                let is_relay = info
                    .protocols
                    .iter()
                    .any(|protocol| *protocol == relay::HOP_PROTOCOL_NAME);

                for addr in info.listen_addrs {
                    swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());

                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

                    if is_relay {
                        let listen_addr =
                            addr.with_p2p(peer_id).unwrap().with(Protocol::P2pCircuit);

                        info!(target: LOG_TARGET, "Trying to listen on relay address {}", listen_addr);

                        swarm.listen_on(listen_addr).map_err(|error| {
                            ServerError::P2pNetwork(P2pNetworkError::Libp2pTransport(error))
                        })?;
                    }
                }
            }
            _ => Self::log_debug(&event),
        }
        Ok(())
    }
}

#[async_trait]
impl Service for P2pService {
    async fn start(&self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let mut swarm = self.swarm().await?;

        for addr in ["/ip4/0.0.0.0/tcp/0", "/ip4/0.0.0.0/udp/0/quic-v1"] {
            swarm
                .listen_on(addr.parse().map_err(|error| {
                    ServerError::P2pNetwork(P2pNetworkError::Libp2pMultiAddrParse(error))
                })?)
                .map_err(|error| {
                    ServerError::P2pNetwork(P2pNetworkError::Libp2pTransport(error))
                })?;
        }

        swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

        let file_owners_topic = IdentTopic::new("available_files");
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&file_owners_topic)
            .map_err(|error| {
                ServerError::P2pNetwork(P2pNetworkError::Libp2pGossipsubSubscription(error))
            })?;

        loop {
            select! {
                event = swarm.select_next_some() => Self::handle_swarm_event(&mut swarm, event)?,
                _ = cancellation_token.cancelled() => {
                    info!(target: LOG_TARGET, "P2P networking service is shutting down because it received the shutdown signal.");
                    break
                },
            }
        }

        Ok(())
    }
}
