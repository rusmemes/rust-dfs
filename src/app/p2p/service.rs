use crate::app::errors::ServerError;
use crate::app::file_store::FileStore;
use crate::app::p2p::config::P2pServiceConfig;
use crate::app::p2p::domain::P2pNetworkBehaviour;
use crate::app::p2p::errors::P2pNetworkError;
use crate::app::p2p::events::{file_publish, handle_swarm_event};
use crate::app::server::Service;
use async_trait::async_trait;
use libp2p::futures::StreamExt;
use libp2p::gossipsub::IdentTopic;
use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::Mode;
use libp2p::relay::client::Behaviour;
use libp2p::request_response::cbor;
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, request_response, tcp,
    yamux, StreamProtocol, Swarm,
};
use log::info;
use std::error::Error;
use std::time::Duration;
use tokio::select;
use tokio_util::sync::CancellationToken;

const LOG_TARGET: &str = "app::p2p::service";

pub struct P2pService<S: FileStore> {
    config: P2pServiceConfig,
    store: S,
}

impl<S: FileStore> P2pService<S> {
    pub fn new(config: P2pServiceConfig, store: S) -> Self {
        Self { config, store }
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
            .with_behaviour(with_behaviour)
            .map_err(|e| P2pNetworkError::Libp2pSwarmBuilder(e.to_string()))?
            .with_swarm_config(|config| {
                config.with_idle_connection_timeout(Duration::from_secs(30))
            })
            .build())
    }
}

fn with_behaviour(
    key_pair: &Keypair,
    relay_client: Behaviour,
) -> Result<P2pNetworkBehaviour, Box<dyn Error + Send + Sync>> {
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
        mdns: mdns::Behaviour::new(mdns::Config::default(), key_pair.public().to_peer_id())?,
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
        metadata_download: cbor::Behaviour::new(
            [(
                StreamProtocol::new("/dfs/1.0.0/metadata-download"),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
    })
}

#[async_trait]
impl<S: FileStore> Service for P2pService<S> {
    async fn start(&mut self, cancellation_token: CancellationToken) -> Result<(), ServerError> {
        let mut swarm = self.swarm().await?;

        for addr in ["/ip4/0.0.0.0/tcp/0", "/ip4/0.0.0.0/udp/0/quic-v1"] {
            swarm
                .listen_on(
                    addr.parse()
                        .map_err(|error| P2pNetworkError::Libp2pMultiAddrParse(error))?,
                )
                .map_err(|error| P2pNetworkError::Libp2pTransport(error))?;
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
                result = self.store.get_next_file_processing_result() => file_publish(&self.store, &mut swarm, result, &file_owners_topic).await,
                event = swarm.select_next_some() => handle_swarm_event(&self.store, &mut swarm, event).await,
                _ = cancellation_token.cancelled() => {
                    info!(target: LOG_TARGET, "P2P networking service is shutting down because it received the shutdown signal.");
                    break
                },
            }
        }

        Ok(())
    }
}
