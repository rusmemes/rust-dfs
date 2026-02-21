use crate::app::service::domain::P2pNetworkBehaviour;
use crate::app::{FileChunkRequest, FileChunkResponse, P2pNetworkBehaviourEvent, ServerError};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{gossipsub, identify, mdns, relay, request_response, Swarm};
use log::{debug, info, warn};
use std::fmt::Debug;

const LOG_TARGET: &str = "app::p2p::events";

pub fn handle_swarm_event(
    swarm: &mut Swarm<P2pNetworkBehaviour>,
    event: SwarmEvent<P2pNetworkBehaviourEvent>,
) -> Result<(), ServerError> {
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
            FileDownload(event) => file_download(swarm, event)?,
            _ => log_debug(&event),
        },
        NewListenAddr {
            listener_id: _listener_id,
            address,
        } => info!(target: LOG_TARGET, "Listening on {:?}", address),
        _ => log_debug(&event),
    }

    Ok(())
}

fn file_download(
    _swarm: &mut Swarm<P2pNetworkBehaviour>,
    event: request_response::Event<FileChunkRequest, FileChunkResponse>,
) -> Result<(), ServerError> {
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
    Ok(())
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
        Discovered(new_peers) => {
            for (peer_id, addr) in new_peers {
                info!(target: LOG_TARGET, "[mDNS] Discovered peer {:?} at {:?}", peer_id, addr);
                swarm.add_peer_address(peer_id, addr.clone());
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
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
                    let listen_addr = addr.with_p2p(peer_id).unwrap().with(Protocol::P2pCircuit);

                    info!(target: LOG_TARGET, "Trying to listen on relay address {}", listen_addr);

                    if let Err(error) = swarm.listen_on(listen_addr.clone()) {
                        warn!(target: LOG_TARGET, "Error listen on relay using address {}: {}", listen_addr, error);
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
