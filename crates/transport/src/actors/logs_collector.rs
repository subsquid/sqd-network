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

use sqd_messages::{QueryExecuted, QueryLogs};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        request_server::{Request, ServerBehaviour},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::{ProtoCodec, ACK_SIZE},
    protocol::GATEWAY_LOGS_PROTOCOL,
    record_event,
    util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogsCollectorEvent {
    /// Worker reports executed queries (a bundle)
    WorkerLogs {
        peer_id: PeerId,
        logs: Vec<QueryExecuted>,
    },
}

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LogsCollectorConfig {
    pub logs_collected_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub logs_ack_timeout: Duration,
}

impl Default for LogsCollectorConfig {
    fn default() -> Self {
        Self {
            logs_collected_queue_size: 100,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            logs_ack_timeout: Duration::from_secs(5),
        }
    }
}

pub struct LogsCollectorBehaviour {
    inner: InnerBehaviour,
}

impl LogsCollectorBehaviour {
    pub fn new(
        base: BaseBehaviour,
        _local_peer_id: PeerId,
        _config: LogsCollectorConfig,
    ) -> Wrapped<Self> {
        Self {
            inner: InnerBehaviour { base: base.into() },
        }
        .into()
    }

    fn on_worker_logs(&mut self, peer_id: PeerId, logs: QueryLogs) -> Option<LogsCollectorEvent> {
        let mut logs = logs.queries_executed;
        log::debug!("Got {} query logs from {peer_id}", logs.len());
        logs = logs
            .into_iter()
            .filter(|log| {
                if log.verify_client_signature(peer_id) {
                    true
                } else {
                    log::warn!("Invalid client signature in query log: {log:?}");
                    false
                }
            })
            .collect();
        (!logs.is_empty()).then_some(LogsCollectorEvent::WorkerLogs { peer_id, logs })
    }
}

impl BehaviourWrapper for LogsCollectorBehaviour {
    type Inner = InnerBehaviour;
    type Event = LogsCollectorEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            _ => None,
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct LogsCollectorTransport {
    swarm: Swarm<Wrapped<LogsCollectorBehaviour>>,
    events_tx: Sender<LogsCollectorEvent>,
}

impl LogsCollectorTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting logs collector P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down logs collector P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<LogsCollectorEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct LogsCollectorTransportHandle {
    _task_manager: Arc<TaskManager>,
}

impl LogsCollectorTransportHandle {
    fn new(transport: LogsCollectorTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<LogsCollectorBehaviour>>,
    config: LogsCollectorConfig,
) -> (impl Stream<Item = LogsCollectorEvent>, LogsCollectorTransportHandle) {
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = LogsCollectorTransport { swarm, events_tx };
    let handle = LogsCollectorTransportHandle::new(transport, config.shutdown_timeout);
    (events_rx, handle)
}
