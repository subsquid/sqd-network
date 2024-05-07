use std::{collections::HashSet, sync::Arc, time::Duration};

use futures::StreamExt;

use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use subsquid_messages::{broadcast_msg, envelope, BroadcastMsg, Envelope, Ping, Pong};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent, ACK_SIZE},
        request_client::{ClientBehaviour, ClientConfig, ClientEvent},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    protocol::{MAX_PONG_SIZE, PONG_PROTOCOL},
    record_event,
    util::{TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchedulerEvent {
    /// Ping received from a worker
    Ping { peer_id: PeerId, ping: Ping },
    /// Peer was probed for reachability
    PeerProbed { peer_id: PeerId, reachable: bool },
}

type PongBehaviour = Wrapped<ClientBehaviour<ProtoCodec<Pong, u32>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    pong: PongBehaviour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub pong_config: ClientConfig,
    pub max_pong_size: u64,
    pub pongs_queue_size: usize,
    pub probes_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            pong_config: Default::default(),
            max_pong_size: MAX_PONG_SIZE,
            pongs_queue_size: 1000,
            probes_queue_size: 1000,
            events_queue_size: 1000,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct SchedulerBehaviour {
    inner: InnerBehaviour,
    legacy_workers: HashSet<PeerId>,
}

impl SchedulerBehaviour {
    pub fn new(mut base: BaseBehaviour, config: SchedulerConfig) -> Wrapped<Self> {
        base.subscribe_pings();
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                pong: ClientBehaviour::new(
                    ProtoCodec::new(config.max_pong_size, ACK_SIZE),
                    PONG_PROTOCOL,
                    config.pong_config,
                )
                .into(),
            },
            legacy_workers: Default::default(),
        }
        .into()
    }

    #[rustfmt::skip]
    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<SchedulerEvent> {
        match ev {
            BaseBehaviourEvent::BroadcastMsg {
                peer_id,
                msg:  BroadcastMsg{ msg: Some(broadcast_msg::Msg::Ping(ping)) },
            } => {
                self.legacy_workers.remove(&peer_id);
                self.on_ping(peer_id, ping)
            },
            BaseBehaviourEvent::LegacyMsg {
                peer_id,
                envelope: Envelope { msg: Some(envelope::Msg::Ping(ping))},
            } => {
                self.legacy_workers.insert(peer_id);
                self.on_ping(peer_id, ping)
            },
            BaseBehaviourEvent::PeerProbed { peer_id, reachable } => self.on_peer_probed(peer_id, reachable),
            _ => None
        }
    }

    fn on_ping(&mut self, peer_id: PeerId, ping: Ping) -> Option<SchedulerEvent> {
        log::debug!("Got ping from {peer_id}: {ping:?}");
        match ping.worker_id.as_ref().map(|id| id.parse::<PeerId>()) {
            Some(Ok(worker_id)) if worker_id == peer_id => {}
            _ => {
                log::warn!("Dropping ping with invalid peer_id");
                return None;
            }
        }
        Some(SchedulerEvent::Ping { peer_id, ping })
    }

    fn on_peer_probed(&mut self, peer_id: PeerId, reachable: bool) -> Option<SchedulerEvent> {
        log::debug!("Peer {peer_id} probed reachable={reachable}");
        Some(SchedulerEvent::PeerProbed { peer_id, reachable })
    }

    fn on_pong_event(&mut self, ev: ClientEvent<u32>) -> Option<SchedulerEvent> {
        match ev {
            ClientEvent::Response { .. } => {} // response is just ACK, no useful information
            ClientEvent::PeerUnknown { peer_id } => self.inner.base.find_and_dial(peer_id),
            ClientEvent::Timeout { peer_id, .. } => log::warn!("Sending pong to {peer_id} failed"),
        }
        None
    }

    pub fn send_pong(&mut self, peer_id: PeerId, pong: Pong) {
        log::debug!("Sending pong to {peer_id}");
        if self.legacy_workers.contains(&peer_id) {
            return self.inner.base.send_legacy_msg(&peer_id, pong);
        }
        if self.inner.pong.try_send_request(peer_id, pong).is_err() {
            log::error!("Cannot send pong to {peer_id}: outbound queue full")
        }
    }

    pub fn try_probe_peer(&mut self, peer_id: PeerId) -> Result<bool, QueueFull> {
        self.inner.base.try_probe_peer(peer_id)
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
            InnerBehaviourEvent::Pong(ev) => self.on_pong_event(ev),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct SchedulerTransport {
    swarm: Swarm<Wrapped<SchedulerBehaviour>>,
    pongs_rx: mpsc::Receiver<(PeerId, Pong)>,
    probes_rx: mpsc::Receiver<PeerId>,
    events_tx: mpsc::Sender<SchedulerEvent>,
}

impl SchedulerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting scheduler P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some((peer_id, pong)) = self.pongs_rx.recv() => self.swarm.behaviour_mut().send_pong(peer_id, pong),
                Some(peer_id) = self.probes_rx.recv() => self.probe_peer(peer_id),
            }
        }
        log::info!("Shutting down scheduler P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<SchedulerEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx
                .try_send(ev)
                .unwrap_or_else(|e| log::error!("Scheduler event queue full. Event dropped: {e:?}"))
        }
    }

    fn probe_peer(&mut self, peer_id: PeerId) {
        log::debug!("Probing peer {peer_id}");
        match self.swarm.behaviour_mut().try_probe_peer(peer_id) {
            Ok(true) => self
                .events_tx
                .try_send(SchedulerEvent::PeerProbed {
                    peer_id,
                    reachable: true,
                })
                .unwrap_or_else(|_| {
                    log::error!("Scheduler event queue full. Probe result dropped.")
                }),

            Ok(false) => {}
            Err(_) => log::error!("Too many active probes. Cannot schedule next one."),
        }
    }
}

#[derive(Clone)]
pub struct SchedulerTransportHandle {
    pongs_tx: mpsc::Sender<(PeerId, Pong)>,
    probes_tx: mpsc::Sender<PeerId>,
    _task_manager: Arc<TaskManager>,
}

impl SchedulerTransportHandle {
    fn new(
        pongs_tx: mpsc::Sender<(PeerId, Pong)>,
        probes_tx: mpsc::Sender<PeerId>,
        transport: SchedulerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            pongs_tx,
            probes_tx,
            _task_manager: Arc::new(task_manager),
        }
    }
    pub fn send_pong(&self, peer_id: PeerId, pong: Pong) -> Result<(), QueueFull> {
        log::debug!("Queueing pong to {peer_id}: {pong:?}");
        Ok(self.pongs_tx.try_send((peer_id, pong))?)
    }

    pub fn probe_peer(&self, peer_id: PeerId) -> Result<(), QueueFull> {
        log::debug!("Queueing probe of peer {peer_id}");
        Ok(self.probes_tx.try_send(peer_id)?)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<SchedulerBehaviour>>,
    config: SchedulerConfig,
) -> (impl Stream<Item = SchedulerEvent>, SchedulerTransportHandle) {
    let (pongs_tx, pongs_rx) = mpsc::channel(config.pongs_queue_size);
    let (probes_tx, probes_rx) = mpsc::channel(config.probes_queue_size);
    let (events_tx, events_rx) = mpsc::channel(config.events_queue_size);
    let transport = SchedulerTransport {
        swarm,
        pongs_rx,
        probes_rx,
        events_tx,
    };
    let handle =
        SchedulerTransportHandle::new(pongs_tx, probes_tx, transport, config.shutdown_timeout);
    (ReceiverStream::new(events_rx), handle)
}
