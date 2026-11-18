use std::{collections::HashSet, num::NonZeroUsize, sync::Arc};

use libp2p::{
    core::ConnectedPoint,
    swarm::{
        behaviour::ConnectionEstablished,
        dial_opts::{DialOpts, WithPeerId},
        ConnectionClosed, ConnectionId, DialError, DialFailure, FromSwarm,
    },
    PeerId,
};
use lru::LruCache;
use parking_lot::Mutex;

/// A peer pushed out of this many just redials from the listen port.
const MAX_FAILED_PEERS: NonZeroUsize = NonZeroUsize::new(1024).unwrap();

/// Redials a peer from a new local port after a dial to it failed in the transport.
///
/// QUIC dials go out from the listen socket by default, so every retry to a peer uses the same
/// UDP 5-tuple. A stale NAT or conntrack entry for that tuple drops every retry, and the retries
/// keep the entry from expiring. A new port gives the retry a new tuple.
///
/// Each new-port dial binds its own UDP socket and QUIC endpoint, which stay open as long as the
/// connection it establishes. At most `max_connections` such dials and connections exist at once;
/// past that, dials reuse the listen port.
///
/// Clones share state. Exactly one behaviour feeds it swarm events.
#[derive(Clone)]
pub struct NewPortRedials(Arc<Mutex<State>>);

struct State {
    /// Peers whose last dial failed in the transport, with no outbound connection since.
    failed: LruCache<PeerId, ()>,
    /// New-port dials in flight and the connections they established.
    active: HashSet<ConnectionId>,
    max_active: usize,
}

impl NewPortRedials {
    pub fn new(max_connections: usize) -> Self {
        Self(Arc::new(Mutex::new(State {
            failed: LruCache::new(MAX_FAILED_PEERS),
            active: HashSet::new(),
            max_active: max_connections,
        })))
    }

    /// Builds a dial to `peer`, from a new port if its last dial failed in the transport.
    pub fn dial_opts(&self, peer: PeerId, opts: WithPeerId) -> DialOpts {
        let mut state = self.0.lock();
        if !state.failed.contains(&peer) || state.active.len() >= state.max_active {
            return opts.build();
        }
        let opts = opts.allocate_new_port().build();
        state.active.insert(opts.connection_id());
        opts
    }

    pub fn on_swarm_event(&self, event: FromSwarm) {
        match event {
            FromSwarm::DialFailure(DialFailure {
                peer_id,
                error,
                connection_id,
            }) => {
                let mut state = self.0.lock();
                state.active.remove(&connection_id);
                if let (Some(peer), DialError::Transport(_)) = (peer_id, error) {
                    state.failed.put(peer, ());
                }
            }
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                endpoint: ConnectedPoint::Dialer { .. },
                ..
            }) => {
                self.0.lock().failed.pop(&peer_id);
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { connection_id, .. }) => {
                self.0.lock().active.remove(&connection_id);
            }
            _ => {}
        }
    }
}

/// `DialOpts` keeps its port choice private; its `Debug` output is the only way to read it.
#[cfg(test)]
pub(crate) fn port_use(opts: &DialOpts) -> libp2p::core::transport::PortUse {
    use libp2p::core::transport::PortUse;

    let debug = format!("{opts:?}");
    if debug.contains("port_use: New") {
        PortUse::New
    } else if debug.contains("port_use: Reuse") {
        PortUse::Reuse
    } else {
        panic!("no port choice in {debug}")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use libp2p::{
        core::{transport::PortUse, Endpoint},
        Multiaddr, TransportError,
    };

    use super::*;

    pub(crate) fn transport_error() -> DialError {
        DialError::Transport(vec![(
            "/ip4/1.2.3.4/udp/12345/quic-v1".parse().expect("valid addr"),
            TransportError::Other(std::io::Error::other("handshake timed out")),
        )])
    }

    pub(crate) fn dial_failure(
        redials: &NewPortRedials,
        peer: PeerId,
        connection_id: ConnectionId,
        error: &DialError,
    ) {
        redials.on_swarm_event(FromSwarm::DialFailure(DialFailure {
            peer_id: Some(peer),
            error,
            connection_id,
        }));
    }

    pub(crate) fn dialer() -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address: Multiaddr::empty(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    }

    fn established(redials: &NewPortRedials, peer: PeerId, connection_id: ConnectionId) {
        redials.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: peer,
            connection_id,
            endpoint: &dialer(),
            failed_addresses: &[],
            other_established: 0,
        }));
    }

    fn closed(redials: &NewPortRedials, peer: PeerId, connection_id: ConnectionId) {
        redials.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id: peer,
            connection_id,
            endpoint: &dialer(),
            cause: None,
            remaining_established: 0,
        }));
    }

    fn dial(redials: &NewPortRedials, peer: PeerId) -> DialOpts {
        redials.dial_opts(peer, DialOpts::peer_id(peer))
    }

    /// After a transport failure the address is evicted, so the next dial can fail with
    /// `NoAddresses` before a lookup finds it again. The dial that follows the lookup is the one
    /// that needs the new port.
    #[test]
    fn a_dial_that_reaches_no_address_keeps_the_new_port() {
        let redials = NewPortRedials::new(4);
        let peer = PeerId::random();
        dial_failure(&redials, peer, ConnectionId::new_unchecked(1), &transport_error());

        let no_address = dial(&redials, peer);
        dial_failure(&redials, peer, no_address.connection_id(), &DialError::NoAddresses);

        assert_eq!(port_use(&dial(&redials, peer)), PortUse::New);
    }

    /// Each new-port connection holds its own socket and QUIC endpoint, so they are capped. A
    /// slot frees up when its dial fails or its connection closes.
    #[test]
    fn new_port_dials_past_the_cap_reuse_the_listen_port() {
        let redials = NewPortRedials::new(1);
        let [first, second] = [PeerId::random(), PeerId::random()];
        for peer in [first, second] {
            dial_failure(&redials, peer, ConnectionId::new_unchecked(1), &transport_error());
        }

        let failing = dial(&redials, first);
        assert_eq!(port_use(&failing), PortUse::New);
        assert_eq!(port_use(&dial(&redials, second)), PortUse::Reuse, "cap not applied");

        dial_failure(&redials, first, failing.connection_id(), &transport_error());
        let connecting = dial(&redials, second);
        assert_eq!(port_use(&connecting), PortUse::New, "a failed dial kept its slot");

        established(&redials, second, connecting.connection_id());
        assert_eq!(port_use(&dial(&redials, first)), PortUse::Reuse, "cap not applied");

        closed(&redials, second, connecting.connection_id());
        assert_eq!(
            port_use(&dial(&redials, first)),
            PortUse::New,
            "a closed connection kept its slot"
        );
    }
}
