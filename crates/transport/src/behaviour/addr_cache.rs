use std::{
    collections::HashSet,
    iter,
    num::NonZeroUsize,
    task::{Context, Poll},
};

use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        dummy::ConnectionHandler, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour,
        THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use lru::LruCache;

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
        self.cache.get_or_insert_mut(peer_id, Default::default).extend(addrs)
    }

    pub fn contains(&self, peer_id: &PeerId) -> bool {
        self.cache.contains(peer_id)
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
        if let FromSwarm::NewExternalAddrOfPeer(e) = event {
            self.put(e.peer_id, iter::once(e.addr.clone()))
        }
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
