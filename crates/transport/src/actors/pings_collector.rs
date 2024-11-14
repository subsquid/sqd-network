use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    Swarm,
};
use serde::{Deserialize, Serialize};
use sqd_messages::OldPing;
use tokio_util::sync::CancellationToken;

use sqd_contract_client::PeerId;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    record_event,
    util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub peer_id: PeerId,
    pub heartbeat: sqd_messages::Heartbeat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PingsCollectorConfig {
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl Default for PingsCollectorConfig {
    fn default() -> Self {
        Self {
            events_queue_size: 1000,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct PingsCollectorBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl PingsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.subscribe_old_pings();
        base.subscribe_heartbeats();
        Self { base: base.into() }.into()
    }
}

impl BehaviourWrapper for PingsCollectorBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = Heartbeat;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.base
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            BaseBehaviourEvent::Heartbeat { peer_id, heartbeat } => {
                log::debug!("Got heartbeat from {peer_id}");
                log::trace!("{heartbeat:?}");
                Some(ToSwarm::GenerateEvent(Heartbeat { peer_id, heartbeat }))
            }
            BaseBehaviourEvent::OldPing { peer_id, ping } => {
                log::debug!("Got old ping from {peer_id}");
                log::trace!("{ping:?}");
                Some(ToSwarm::GenerateEvent(Heartbeat {
                    peer_id,
                    heartbeat: convert_old_ping(ping),
                }))
            }
            _ => None,
        }
    }
}

// TODO: drop support for old pings
fn convert_old_ping(ping: OldPing) -> sqd_messages::Heartbeat {
    sqd_messages::Heartbeat {
        version: ping.version.unwrap_or_default(),
        stored_bytes: ping.stored_bytes,
        ..Default::default()
    }
}

struct PingsCollectorTransport {
    swarm: Swarm<Wrapped<PingsCollectorBehaviour>>,
    heartbeats_tx: Sender<Heartbeat>,
}

impl PingsCollectorTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting pings collector P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down pings collector P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<Heartbeat>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(heartbeat) = ev {
            self.heartbeats_tx.send_lossy(heartbeat)
        }
    }
}

#[derive(Clone)]
pub struct PingsCollectorTransportHandle {
    _task_manager: Arc<TaskManager>,
}

impl PingsCollectorTransportHandle {
    fn new(transport: PingsCollectorTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PingsCollectorBehaviour>>,
    config: PingsCollectorConfig,
) -> (impl Stream<Item = Heartbeat>, PingsCollectorTransportHandle) {
    let (heartbeats_tx, heartbeats_rx) = new_queue(config.events_queue_size, "events");
    let transport = PingsCollectorTransport {
        swarm,
        heartbeats_tx,
    };
    let handle = PingsCollectorTransportHandle::new(transport, config.shutdown_timeout);
    (heartbeats_rx, handle)
}
