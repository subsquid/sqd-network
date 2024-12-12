use std::{sync::Arc, time::Duration};

use futures::StreamExt;

use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::Heartbeat;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent, PeerProbed, ProbeResult, TryProbeError},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchedulerEvent {
    /// Heartbeat received from a worker
    Heartbeat {
        peer_id: PeerId,
        heartbeat: Heartbeat,
    },
    /// Peer was probed for reachability
    PeerProbed { peer_id: PeerId, reachable: bool },
}

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Maximum number of pending peer probes (reachability checks) stored in a queue (default: 1000)
    pub probes_queue_size: usize,
    /// Maximum number of inbound events (`SchedulerEvent`) stored in a queue (default: 1000)
    pub events_queue_size: usize,
    /// Should existing outbound connections be ignored when probing peers (default: false)
    pub ignore_existing_conns: bool,
    /// Shutdown timeout for the transport loop (default: `DEFAULT_SHUTDOWN_TIMEOUT`)
    pub shutdown_timeout: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            probes_queue_size: 1000,
            events_queue_size: 1000,
            ignore_existing_conns: false,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct SchedulerBehaviour {
    inner: InnerBehaviour,
}

impl SchedulerBehaviour {
    pub fn new(mut base: BaseBehaviour, _config: SchedulerConfig) -> Wrapped<Self> {
        base.subscribe_heartbeats();
        Self {
            inner: InnerBehaviour { base: base.into() },
        }
        .into()
    }

    #[rustfmt::skip]
    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<SchedulerEvent> {
        match ev {
            BaseBehaviourEvent::Heartbeat { peer_id, heartbeat } => self.on_heartbeat(peer_id, heartbeat),
            BaseBehaviourEvent::PeerProbed(PeerProbed { peer_id, result }) => self.on_peer_probed(peer_id, &result),
        }
    }

    fn on_heartbeat(&mut self, peer_id: PeerId, heartbeat: Heartbeat) -> Option<SchedulerEvent> {
        log::debug!("Got heartbeat from {peer_id}");
        log::trace!("{heartbeat:?}");
        Some(SchedulerEvent::Heartbeat { peer_id, heartbeat })
    }

    fn on_peer_probed(&mut self, peer_id: PeerId, result: &ProbeResult) -> Option<SchedulerEvent> {
        log::debug!("Peer {peer_id} probed result={result:?}");
        let reachable = matches!(result, ProbeResult::Reachable { .. });
        Some(SchedulerEvent::PeerProbed { peer_id, reachable })
    }

    pub fn try_probe_peer(&mut self, peer_id: PeerId) -> Result<(), TryProbeError> {
        self.inner.base.try_probe_dht(peer_id)
    }
}

impl BehaviourWrapper for SchedulerBehaviour {
    type Inner = InnerBehaviour;
    type Event = SchedulerEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            InnerBehaviourEvent::Base(ev) => self.on_base_event(ev),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct SchedulerTransport {
    swarm: Swarm<Wrapped<SchedulerBehaviour>>,
    probes_rx: Receiver<PeerId>,
    events_tx: Sender<SchedulerEvent>,
    ignore_existing_conns: bool,
}

impl SchedulerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting scheduler P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(peer_id) = self.probes_rx.recv() => self.probe_peer(peer_id),
            }
        }
        log::info!("Shutting down scheduler P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<SchedulerEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }

    fn probe_peer(&mut self, peer_id: PeerId) {
        if !self.ignore_existing_conns
            && self.swarm.behaviour().inner.base.outbound_conn_exists(&peer_id)
        {
            log::info!("Outbound connection to {peer_id} already exists. Skipping probe");
            self.events_tx.send_lossy(SchedulerEvent::PeerProbed {
                peer_id,
                reachable: true,
            })
        } else {
            log::info!("Probing peer {peer_id}");
            self.swarm
                .behaviour_mut()
                .try_probe_peer(peer_id)
                .unwrap_or_else(|e| log::error!("{e}"));
        }
    }
}

#[derive(Clone)]
pub struct SchedulerTransportHandle {
    probes_tx: Sender<PeerId>,
    _task_manager: Arc<TaskManager>,
}

impl SchedulerTransportHandle {
    fn new(
        probes_tx: Sender<PeerId>,
        transport: SchedulerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            probes_tx,
            _task_manager: Arc::new(task_manager),
        }
    }

    pub fn probe_peer(&self, peer_id: PeerId) -> Result<(), QueueFull> {
        log::debug!("Queueing probe of peer {peer_id}");
        self.probes_tx.try_send(peer_id)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<SchedulerBehaviour>>,
    config: SchedulerConfig,
) -> (impl Stream<Item = SchedulerEvent>, SchedulerTransportHandle) {
    let (probes_tx, probes_rx) = new_queue(config.probes_queue_size, "probes");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = SchedulerTransport {
        swarm,
        probes_rx,
        events_tx,
        ignore_existing_conns: config.ignore_existing_conns,
    };
    let handle = SchedulerTransportHandle::new(probes_tx, transport, config.shutdown_timeout);
    (events_rx, handle)
}
