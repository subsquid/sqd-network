use futures::{AsyncReadExt, StreamExt};
use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
    StreamProtocol,
};
use std::task::{Context, Poll};

use crate::{protocol::KEEP_ALIVE_PROTOCOL, Multiaddr, PeerId};

/// A behaviour that allows keeping connections alive forever.
///
/// The protocol is very simple — any incoming stream is accepted and held open indefinitely.
/// This prevents the connection from being closed due to idle timeout, as an open substream
/// counts as active protocol usage.
pub struct KeepAliveBehaviour {
    stream: libp2p_stream::Behaviour,
    control: libp2p_stream::Control,
    incoming: libp2p_stream::IncomingStreams,
    active: bool,
}

impl Default for KeepAliveBehaviour {
    fn default() -> Self {
        let stream = libp2p_stream::Behaviour::new();
        let mut control = stream.new_control();
        let incoming = control
            .accept(StreamProtocol::new(KEEP_ALIVE_PROTOCOL))
            .expect("KeepAlive listener should not already exist");
        Self {
            stream,
            control,
            incoming,
            active: false,
        }
    }
}

impl KeepAliveBehaviour {
    /// Request that all connections are kept alive.
    ///
    /// By default the behaviour works in server mode, only accepting incoming keep-alive requests.
    /// This method switches the behaviour to client mode, requesting to keep all established connections alive.
    pub fn keep_all_connections_alive(&mut self) {
        self.active = true;
    }

    fn ensure_keep_alive(&self, peer: PeerId) {
        if !self.active {
            return;
        }

        let mut control = self.control.clone();
        tokio::spawn(async move {
            match control.open_stream(peer, StreamProtocol::new(KEEP_ALIVE_PROTOCOL)).await {
                Ok(mut stream) => {
                    let mut buf = [0; 1];
                    let _ = stream.read(&mut buf).await;
                    log::debug!("Keep-alive stream to {peer} closed");
                }
                Err(e) => {
                    log::info!("Failed to open keep-alive stream to {peer}: {e}");
                    return;
                }
            };
        });
    }
}

impl NetworkBehaviour for KeepAliveBehaviour {
    type ConnectionHandler = <libp2p_stream::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = std::convert::Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.stream
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        let result = self.stream.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        );
        self.ensure_keep_alive(peer);
        result
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.stream.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        let result = self.stream.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        );
        self.ensure_keep_alive(peer);
        result
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        self.stream.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.stream.on_connection_handler_event(peer_id, connection_id, event)
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Poll incoming streams
        loop {
            match futures::ready!(self.incoming.poll_next_unpin(cx)) {
                Some((peer, mut stream)) => {
                    log::debug!("Accepted keep-alive request from {peer}");
                    tokio::spawn(async move {
                        use futures::AsyncReadExt;
                        let mut buf = [0u8; 1];
                        match stream.read(&mut buf).await {
                            Ok(0) => {
                                log::debug!("Keep-alive stream from {peer} closed");
                            }
                            Ok(_) => {
                                log::info!(
                                    "Keep-alive stream from {peer} violated the protocol, closing"
                                );
                            }
                            Err(e) => {
                                log::debug!(
                                    "Keep-alive stream from {peer} closed unexpectedly: {e}"
                                );
                            }
                        }
                    });
                }
                None => {
                    log::warn!("Keep-alive incoming stream ended unexpectedly");
                    break;
                }
            }
        }

        // Poll inner behaviour
        match self.stream.poll(cx) {
            Poll::Ready(event) => Poll::Ready(
                event.map_out(|()| unreachable!("Stream behaviour doesn't produce events")),
            ),
            Poll::Pending => Poll::Pending,
        }
    }
}
