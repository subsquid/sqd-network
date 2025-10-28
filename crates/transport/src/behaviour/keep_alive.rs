use libp2p::swarm::{
    ConnectionHandler, ConnectionHandlerEvent, SubstreamProtocol,
};
use libp2p::swarm::{NetworkBehaviour, ToSwarm};
use std::task::{Context, Poll};

/// This behaviour does nothing except preventing libp2p's idle connection timeout
/// from closing connections.
#[derive(Default, Debug)]
pub struct KeepAliveBehaviour;

impl NetworkBehaviour for KeepAliveBehaviour {
    type ConnectionHandler = KeepAliveHandler;
    type ToSwarm = std::convert::Infallible;

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: libp2p::swarm::ConnectionId,
        _peer: libp2p::PeerId,
        _local_addr: &libp2p::Multiaddr,
        _remote_addr: &libp2p::Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        Ok(KeepAliveHandler)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: libp2p::swarm::ConnectionId,
        _peer: libp2p::PeerId,
        _addr: &libp2p::Multiaddr,
        _role_override: libp2p::core::Endpoint,
        _port_use: libp2p::core::transport::PortUse,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        Ok(KeepAliveHandler)
    }

    fn on_swarm_event(&mut self, _event: libp2p::swarm::FromSwarm) {}

    fn on_connection_handler_event(
        &mut self,
        _peer_id: libp2p::PeerId,
        _connection_id: libp2p::swarm::ConnectionId,
        _event: libp2p::swarm::THandlerOutEvent<Self>,
    ) {
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, libp2p::swarm::THandlerInEvent<Self>>> {
        Poll::Pending
    }
}

#[derive(Debug)]
pub struct KeepAliveHandler;

impl ConnectionHandler for KeepAliveHandler {
    type FromBehaviour = std::convert::Infallible;
    type ToBehaviour = std::convert::Infallible;
    type InboundProtocol = libp2p::core::upgrade::DeniedUpgrade;
    type OutboundProtocol = libp2p::core::upgrade::DeniedUpgrade;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = std::convert::Infallible;

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(libp2p::core::upgrade::DeniedUpgrade, ())
    }

    fn on_behaviour_event(&mut self, _event: Self::FromBehaviour) {}

    fn connection_keep_alive(&self) -> bool {
        // This is the key: return true to prevent idle connection timeout
        true
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>,
    > {
        Poll::Pending
    }

    fn on_connection_event(
        &mut self,
        _event: libp2p::swarm::handler::ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
    }
}