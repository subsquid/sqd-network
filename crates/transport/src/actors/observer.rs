use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::Heartbeat;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    record_event,
    util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObserverEvent {
    Ping { peer_id: PeerId, ping: Heartbeat },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ObserverConfig {
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl ObserverConfig {
    pub fn new() -> Self {
        Self {
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct ObserverBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl ObserverBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.subscribe_heartbeats();
        Self { base: base.into() }.into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<ObserverEvent> {
        match ev {
            BaseBehaviourEvent::Heartbeat {
                peer_id,
                heartbeat: ping,
            } => Some(ObserverEvent::Ping { peer_id, ping }),
            _ => None,
        }
    }
}

impl BehaviourWrapper for ObserverBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = ObserverEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.base
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        self.on_base_event(ev).map(ToSwarm::GenerateEvent)
    }
}

struct ObserverTransport {
    swarm: Swarm<Wrapped<ObserverBehaviour>>,
    events_tx: Sender<ObserverEvent>,
}

impl ObserverTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting observer P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down observer P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<ObserverEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct ObserverTransportHandle {
    _task_manager: Arc<TaskManager>,
}

impl ObserverTransportHandle {
    fn new(transport: ObserverTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<ObserverBehaviour>>,
    config: ObserverConfig,
) -> (impl Stream<Item = ObserverEvent>, ObserverTransportHandle) {
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = ObserverTransport { swarm, events_tx };
    let handle = ObserverTransportHandle::new(transport, config.shutdown_timeout);
    (events_rx, handle)
}
