use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::p2p::domain::{
    FileChunkRequest, FileChunkResponse, P2pNetworkBehaviour, P2pNetworkBehaviourEvent,
};
use crate::app::p2p::models::PublishedFile;
use libp2p::gossipsub::IdentTopic;
use libp2p::kad::{Quorum, Record};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{gossipsub, identify, mdns, relay, request_response, Swarm};
use log::{debug, error, info, warn};
use rs_sha256::Sha256Hasher;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

const LOG_TARGET: &str = "app::p2p::events";

// TODO: what about retries?
pub fn file_publish(
    swarm: &mut Swarm<P2pNetworkBehaviour>,
    file_split_result: FileProcessingResult,
    _topic: &IdentTopic,
) {
    let mut hasher = Sha256Hasher::default();
    file_split_result.hash(&mut hasher);
    let key = hasher.finish().to_be_bytes().to_vec();

    match serde_cbor::to_vec(&PublishedFile {
        total_chunks: file_split_result.total_chunks,
        merkle_root: file_split_result.merkle_root,
    }) {
        Ok(value) => {
            let record = Record::new(key, value);
            let key = record.key.clone();
            if let Err(error) = swarm
                .behaviour_mut()
                .kademlia
                .put_record(record, Quorum::Majority)
            {
                error!("Failed to publish file split record: {}", error);
            } else if let Err(error) = swarm.behaviour_mut().kademlia.start_providing(key) {
                error!("Failed to start providing file split record: {}", error);
            }
        }
        Err(error) => {
            error!(target: LOG_TARGET, "Error serializing file split result: {:?}", error);
        }
    }
}

pub fn handle_swarm_event(
    swarm: &mut Swarm<P2pNetworkBehaviour>,
    event: SwarmEvent<P2pNetworkBehaviourEvent>,
) {
    use P2pNetworkBehaviourEvent::*;
    use SwarmEvent::*;
    match event {
        Behaviour(event) => match event {
            Identify(event) => identify(swarm, event),
            Mdns(event) => mdns(swarm, event),
            Kademlia(event) => log_debug(&event),
            Gossipsub(event) => gossipsub(swarm, event),
            RelayServer(event) => log_debug(&event),
            RelayClient(event) => log_debug(&event),
            Dcutr(event) => log_debug(&event),
            FileDownload(event) => file_download(swarm, event),
            _ => log_debug(&event),
        },
        NewListenAddr {
            listener_id: _listener_id,
            address,
        } => info!(target: LOG_TARGET, "Listening on {:?}", address),
        _ => log_debug(&event),
    }
}

fn file_download(
    _swarm: &mut Swarm<P2pNetworkBehaviour>,
    event: request_response::Event<FileChunkRequest, FileChunkResponse>,
) {
    use request_response::Event::*;
    match event {
        Message {
            peer: _peer,
            connection_id: _connection_id,
            message,
        } => {
            use request_response::Message::*;
            match message {
                Request {
                    request_id: _request_id,
                    request,
                    channel: _channel,
                } => {
                    info!(target: LOG_TARGET, "File download request: {:?}", request);
                }
                Response {
                    request_id: _request_id,
                    response,
                } => {
                    info!(target: LOG_TARGET, "File download response: {:?}", response);
                }
            }
        }
        _ => log_debug(&event),
    }
}

fn gossipsub(_swarm: &mut Swarm<P2pNetworkBehaviour>, event: gossipsub::Event) {
    use gossipsub::Event::*;
    match event {
        Message {
            propagation_source: _propagation_source,
            message_id: _message_id,
            message,
        } => {
            info!(target: LOG_TARGET, "[gossipsub] message: {:?}", message);
        }
        _ => log_debug(&event),
    }
}

fn mdns(swarm: &mut Swarm<P2pNetworkBehaviour>, event: mdns::Event) {
    use mdns::Event::*;

    match event {
        Discovered(peers) => {
            for (peer_id, addr) in peers {
                info!(target: LOG_TARGET, "[mDNS] Discovered {:?} at {:?}", peer_id, addr);

                if is_dialable(&addr) {
                    swarm.add_peer_address(peer_id, addr.clone());
                }

                swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, addr.clone());

                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
            }
        }
        _ => log_debug(&event),
    }
}

fn identify(swarm: &mut Swarm<P2pNetworkBehaviour>, event: identify::Event) {
    use identify::Event::*;

    match event {
        Received {
            connection_id: _,
            peer_id,
            info,
        } => {
            let is_relay = info
                .protocols
                .iter()
                .any(|p| *p == relay::HOP_PROTOCOL_NAME);

            for addr in info.listen_addrs {
                swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, addr.clone());

                if is_dialable(&addr) {
                    swarm.add_peer_address(peer_id, addr.clone());
                }

                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

                if is_relay {
                    if let Ok(relay_addr) = addr
                        .clone()
                        .with_p2p(peer_id)
                        .map(|a| a.with(Protocol::P2pCircuit))
                    {
                        info!(
                            target: LOG_TARGET,
                            "Listening via relay {}",
                            relay_addr
                        );

                        if let Err(e) = swarm.listen_on(relay_addr.clone()) {
                            warn!(
                                target: LOG_TARGET,
                                "Relay listen error on {}: {}",
                                relay_addr,
                                e
                            );
                        }
                    }
                }
            }
        }
        _ => log_debug(&event),
    }
}

fn log_debug<T: Debug>(event: &T) {
    debug!(target: LOG_TARGET, "{:?}", event);
}

fn is_dialable(addr: &libp2p::Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;

    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
            }
            _ => {}
        }
    }

    true
}
