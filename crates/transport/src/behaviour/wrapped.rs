use std::{
    collections::VecDeque,
    fmt::Debug,
    ops::{Deref, DerefMut},
    task::{Context, Poll},
};

use crate::{Multiaddr, PeerId};
use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
};

pub type TToSwarm<T> =
    ToSwarm<<T as BehaviourWrapper>::Event, THandlerInEvent<<T as BehaviourWrapper>::Inner>>;
pub trait BehaviourWrapper {
    type Inner: NetworkBehaviour;
    type Event: Send + 'static;

    fn inner(&mut self) -> &mut Self::Inner;
    fn on_swarm_event(&mut self, _ev: FromSwarm) -> impl IntoIterator<Item = TToSwarm<Self>> {
        None
    }

    fn on_inner_event(
        &mut self,
        _ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        None
    }

    // Usually all the inner commands except for `GenerateEvent` should be passed to the swarm directly.
    // In cases when you explicitly want to intercept those commands, this method may be overridden.
    fn on_inner_command(
        &mut self,
        ev: ToSwarm<<Self::Inner as NetworkBehaviour>::ToSwarm, THandlerInEvent<Self::Inner>>,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            ToSwarm::GenerateEvent(ev) => {
                Box::new(self.on_inner_event(ev).into_iter()) as Box<dyn Iterator<Item = _>>
            }
            ev => Box::new(std::iter::once(ev.map_out::<Self::Event>(|_| unreachable!()))),
        }
    }

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        Poll::<Vec<_>>::Pending
    }
}

pub struct Wrapped<T: BehaviourWrapper + 'static> {
    wrapper: T,
    pending_events: VecDeque<TToSwarm<T>>,
}

impl<T: BehaviourWrapper + 'static> From<T> for Wrapped<T> {
    fn from(wrapper: T) -> Self {
        Self {
            wrapper,
            pending_events: Default::default(),
        }
    }
}

impl<T: BehaviourWrapper + 'static> Deref for Wrapped<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.wrapper
    }
}

impl<T: BehaviourWrapper> DerefMut for Wrapped<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.wrapper
    }
}

impl<T: BehaviourWrapper + 'static> NetworkBehaviour for Wrapped<T>
where
    <T as BehaviourWrapper>::Event: Debug,
    <<T as BehaviourWrapper>::Inner as NetworkBehaviour>::ToSwarm: Debug,
{
    type ConnectionHandler = THandler<T::Inner>;
    type ToSwarm = T::Event;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner()
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner().handle_established_inbound_connection(
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
        self.inner().handle_pending_outbound_connection(
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
        self.inner().handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        self.inner().on_swarm_event(event);
        self.pending_events.extend(self.wrapper.on_swarm_event(event));
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner().on_connection_handler_event(peer_id, connection_id, event)
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        loop {
            if let Some(ev) = self.pending_events.pop_front() {
                log::trace!("Wrapped pending event: {ev:?}");
                return Poll::Ready(ev);
            }
            match self.wrapper.poll(cx) {
                Poll::Ready(events) => {
                    self.pending_events.extend(events.into_iter().inspect(|ev| {
                        log::trace!("Wrapped inner poll event: {ev:?}");
                    }));
                    continue;
                }
                Poll::Pending => {}
            }
            match self.inner().poll(cx) {
                Poll::Ready(ev) => {
                    log::trace!("Wrapped inner behaviour event: {ev:?}");
                    self.pending_events.extend(self.wrapper.on_inner_command(ev));
                    continue;
                }
                Poll::Pending => {}
            }
            return Poll::Pending;
        }
    }
}
