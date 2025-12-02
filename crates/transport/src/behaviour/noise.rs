use futures::StreamExt;
use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
    StreamProtocol,
};
use std::task::{Context, Poll};

use crate::{protocol::NOISE_PROTOCOL, Multiaddr, PeerId};

const BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// A behaviour that infinitely spams random bytes to the requester.
pub struct NoiseBehaviour {
    stream: libp2p_stream::Behaviour,
    incoming: libp2p_stream::IncomingStreams,
}

impl Default for NoiseBehaviour {
    fn default() -> Self {
        let stream = libp2p_stream::Behaviour::new();
        let mut control = stream.new_control();
        let incoming = control
            .accept(StreamProtocol::new(NOISE_PROTOCOL))
            .expect("Noise listener should not already exist");
        Self { stream, incoming }
    }
}

impl NetworkBehaviour for NoiseBehaviour {
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
        self.stream.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )
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
        self.stream.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )
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
            match self.incoming.poll_next_unpin(cx) {
                Poll::Ready(Some((peer, mut stream))) => {
                    log::debug!("Accepted noise request from {peer}");

                    tokio::spawn(async move {
                        use futures::AsyncWriteExt;
                        use rand::RngCore;

                        let mut buffer = vec![0u8; BUFFER_SIZE];
                        rand::rng().fill_bytes(&mut buffer);

                        loop {
                            match stream.write(&buffer).await {
                                Ok(n) => {
                                    log::trace!("Wrote {n} bytes of noise to {peer}");
                                }
                                Err(e) => {
                                    log::debug!(
                                        "Failed to write to noise stream to {peer}: {e}, closing"
                                    );
                                    break;
                                }
                            }
                        }
                        stream.close().await.unwrap();
                    });
                }
                Poll::Ready(None) => {
                    log::warn!("Noise incoming stream ended unexpectedly");
                    break;
                }
                Poll::Pending => break,
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
