use std::{
    collections::HashSet,
    iter,
    num::NonZeroUsize,
    task::{Context, Poll},
};

use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        dummy::ConnectionHandler, ConnectionDenied, ConnectionId, DialFailure, FromSwarm,
        NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use lru::LruCache;

use crate::util::addr_is_reachable;

pub struct AddressCache {
    cache: LruCache<PeerId, HashSet<Multiaddr>>,
}

impl AddressCache {
    pub fn new(size: NonZeroUsize) -> Self {
        Self {
            cache: LruCache::new(size),
        }
    }

    pub fn put(&mut self, peer_id: PeerId, addrs: impl IntoIterator<Item = Multiaddr>) {
        let addrs: Vec<Multiaddr> = addrs
            .into_iter()
            .filter(addr_is_reachable)
            .map(|e| {
                e.with_p2p(peer_id).unwrap_or_else(|e| {
                    log::warn!("Found invalid address for peer {peer_id}: {e}");
                    e
                })
            })
            .inspect(|a| log::trace!("Caching address for peer {peer_id}: {a}"))
            .collect();
        // Never insert an entry with no addresses: `contains` reports such a peer as dialable,
        // which suppresses DHT lookups while every dial fails with `NoAddresses`.
        if addrs.is_empty() {
            return;
        }
        self.cache.get_or_insert_mut(peer_id, Default::default).extend(addrs)
    }

    pub fn evict(&mut self, peer_id: PeerId) {
        self.cache.pop(&peer_id);
    }

    pub fn contains(&self, peer_id: &PeerId) -> bool {
        self.cache.contains(peer_id)
    }

    fn on_dial_failure(&mut self, err: DialFailure) {
        log::debug!("Dial failure: {err:?}");
        match err.error {
            libp2p::swarm::DialError::WrongPeerId { .. }
            | libp2p::swarm::DialError::Transport(_)
            | libp2p::swarm::DialError::NoAddresses => {
                if let Some(peer_id) = err.peer_id {
                    self.evict(peer_id);
                }
            }
            _ => {}
        };
    }
}

impl NetworkBehaviour for AddressCache {
    type ConnectionHandler = ConnectionHandler;
    type ToSwarm = ();

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        _addresses: &[Multiaddr],
        _effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let Some(peer_id) = maybe_peer else {
            return Ok(Vec::new());
        };
        let addrs = self
            .cache
            .get(&peer_id)
            .map(|a| a.iter().cloned().collect())
            .unwrap_or_default();
        Ok(addrs)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        _role_override: Endpoint,
        _port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.put(peer, iter::once(addr.clone()));
        Ok(ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::NewExternalAddrOfPeer(e) => self.put(e.peer_id, iter::once(e.addr.clone())),
            FromSwarm::DialFailure(err) => self.on_dial_failure(err),
            _ => {}
        };
    }

    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        _event: THandlerOutEvent<Self>,
    ) {
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> AddressCache {
        AddressCache::new(NonZeroUsize::new(10).unwrap())
    }

    #[test]
    fn put_with_no_reachable_addrs_does_not_create_entry() {
        let mut cache = cache();
        let peer = PeerId::random();

        cache.put(peer, ["/ip4/127.0.0.1/udp/1000/quic-v1".parse().unwrap()]);
        assert!(!cache.contains(&peer));

        cache.put(peer, std::iter::empty::<Multiaddr>());
        assert!(!cache.contains(&peer));
    }

    #[test]
    fn empty_put_does_not_wipe_existing_entry() {
        let mut cache = cache();
        let peer = PeerId::random();

        cache.put(peer, ["/ip4/1.2.3.4/udp/1000/quic-v1".parse().unwrap()]);
        assert!(cache.contains(&peer));

        cache.put(peer, std::iter::empty::<Multiaddr>());
        assert!(cache.contains(&peer));
    }
}
