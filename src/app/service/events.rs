use crate::app::service::domain::{P2pNetworkBehaviour, P2pNetworkError};
use crate::app::{P2pNetworkBehaviourEvent, ServerError};
use libp2p::identify::Event;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{relay, Swarm};
use log::{debug, info};
use std::fmt::Debug;

const LOG_TARGET: &str = "app::p2p::events";

pub fn handle_swarm_event(
    swarm: &mut Swarm<P2pNetworkBehaviour>,
    event: SwarmEvent<P2pNetworkBehaviourEvent>,
) -> Result<(), ServerError> {
    match event {
        SwarmEvent::Behaviour(event) => match event {
            P2pNetworkBehaviourEvent::Identify(event) => identify(swarm, event)?,
            P2pNetworkBehaviourEvent::Mdns(_) => {}
            P2pNetworkBehaviourEvent::Kademlia(_) => {}
            P2pNetworkBehaviourEvent::Gossipsub(_) => {}
            P2pNetworkBehaviourEvent::RelayServer(_) => {}
            P2pNetworkBehaviourEvent::RelayClient(_) => {}
            P2pNetworkBehaviourEvent::Dcutr(_) => {}
            P2pNetworkBehaviourEvent::FileDownload(_) => {}
            _ => log_debug(&event),
        },
        SwarmEvent::NewListenAddr {
            listener_id: _listener_id,
            address,
        } => info!(target: LOG_TARGET, "Listening on {:?}", address),
        _ => log_debug(&event),
    }

    Ok(())
}

pub fn identify(swarm: &mut Swarm<P2pNetworkBehaviour>, event: Event) -> Result<(), ServerError> {
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
                    let listen_addr = addr.with_p2p(peer_id).unwrap().with(Protocol::P2pCircuit);

                    info!(target: LOG_TARGET, "Trying to listen on relay address {}", listen_addr);

                    swarm.listen_on(listen_addr).map_err(|error| {
                        ServerError::P2pNetwork(P2pNetworkError::Libp2pTransport(error))
                    })?;
                }
            }
        }
        _ => log_debug(&event),
    }
    Ok(())
}

fn log_debug<T: Debug>(event: &T) {
    debug!(target: LOG_TARGET, "{:?}", event);
}
