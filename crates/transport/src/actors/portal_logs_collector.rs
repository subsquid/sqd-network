use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::QueryFinished;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    record_event,
    util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalLogsCollectorEvent {
    Log { peer_id: PeerId, log: QueryFinished },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PortalLogsCollectorConfig {
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl PortalLogsCollectorConfig {
    pub fn new() -> Self {
        Self {
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct PortalLogsCollectorBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl PortalLogsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.subscribe_portal_logs();
        base.claim_portal_logs_listener_role();
        Self { base: base.into() }.into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<PortalLogsCollectorEvent> {
        match ev {
            BaseBehaviourEvent::PortalLogs { peer_id, log }  => Some(PortalLogsCollectorEvent::Log { peer_id, log }),
            _ => None,
        }
    }
}

impl Drop for PortalLogsCollectorBehaviour {
    fn drop(&mut self) {
        self.inner().relinquish_portal_logs_listener_role();
    }
}

impl BehaviourWrapper for PortalLogsCollectorBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = PortalLogsCollectorEvent;

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

struct PortalLogsCollectorTransport {
    swarm: Swarm<Wrapped<PortalLogsCollectorBehaviour>>,
    events_tx: Sender<PortalLogsCollectorEvent>,
}

impl PortalLogsCollectorTransport {
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

    fn on_swarm_event(&mut self, ev: SwarmEvent<PortalLogsCollectorEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct PortalLogsCollectorTransportHandle {
    _task_manager: Arc<TaskManager>,
}

impl PortalLogsCollectorTransportHandle {
    fn new(transport: PortalLogsCollectorTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PortalLogsCollectorBehaviour>>,
    config: PortalLogsCollectorConfig,
) -> (impl Stream<Item = PortalLogsCollectorEvent>, PortalLogsCollectorTransportHandle) {
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = PortalLogsCollectorTransport { swarm, events_tx };
    let handle = PortalLogsCollectorTransportHandle::new(transport, config.shutdown_timeout);
    (events_rx, handle)
}
