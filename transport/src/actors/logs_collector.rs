use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent, ACK_SIZE},
        request_server::{Request, ServerBehaviour},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    util::TaskManager,
    QueueFull,
};
use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use subsquid_messages::{
    envelope, gateway_log_msg, Envelope, GatewayLogMsg, LogsCollected, QueryFinished, QueryLogs,
    QuerySubmitted,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::protocol::{
    GATEWAY_LOGS_PROTOCOL, MAX_GATEWAY_LOG_SIZE, MAX_WORKER_LOGS_SIZE, WORKER_LOGS_PROTOCOL,
};
#[cfg(feature = "metrics")]
use libp2p::metrics::{Metrics, Recorder};
#[cfg(feature = "metrics")]
use prometheus_client::registry::Registry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogsCollectorEvent {
    /// Worker reports executed queries (a bundle)
    WorkerLogs(QueryLogs),
    /// Gateway reports a submitted query
    QuerySubmitted(QuerySubmitted),
    /// Gateway reports a finished query (result received or timeout)
    QueryFinished(QueryFinished),
}

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    worker_logs: Wrapped<ServerBehaviour<ProtoCodec<QueryLogs, u32>>>,
    gateway_logs: Wrapped<ServerBehaviour<ProtoCodec<GatewayLogMsg, u32>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsCollectorConfig {
    pub max_worker_logs_size: u64,
    pub max_gateway_log_size: u64,
    pub logs_collected_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl Default for LogsCollectorConfig {
    fn default() -> Self {
        Self {
            max_worker_logs_size: MAX_WORKER_LOGS_SIZE,
            max_gateway_log_size: MAX_GATEWAY_LOG_SIZE,
            logs_collected_queue_size: 100,
            events_queue_size: 100,
            shutdown_timeout: Duration::from_secs(10),
        }
    }
}

pub struct LogsCollectorBehaviour {
    inner: InnerBehaviour,
}

impl LogsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour, config: LogsCollectorConfig) -> Wrapped<Self> {
        base.subscribe_pings();
        base.subscribe_logs_collected();
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                worker_logs: ServerBehaviour::new(
                    ProtoCodec::new(config.max_worker_logs_size, ACK_SIZE),
                    WORKER_LOGS_PROTOCOL,
                )
                .into(),
                gateway_logs: ServerBehaviour::new(
                    ProtoCodec::new(config.max_gateway_log_size, ACK_SIZE),
                    GATEWAY_LOGS_PROTOCOL,
                )
                .into(),
            },
        }
        .into()
    }
    #[rustfmt::skip]
    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<LogsCollectorEvent> {
        match ev {
            BaseBehaviourEvent::LegacyMsg {
                peer_id,
                envelope: Envelope {msg: Some(envelope::Msg::QueryLogs(logs))}
            } => self.on_worker_logs(peer_id, logs),
            _ => None
        }
    }

    fn on_worker_logs(
        &mut self,
        peer_id: PeerId,
        mut logs: QueryLogs,
    ) -> Option<LogsCollectorEvent> {
        log::debug!("Got {} query logs from {peer_id}", logs.queries_executed.len());
        let worker_id = peer_id.to_base58();
        logs.queries_executed.retain(|log| log.worker_id == worker_id);
        (!logs.queries_executed.is_empty()).then_some(LogsCollectorEvent::WorkerLogs(logs))
    }

    fn on_gateway_log(
        &mut self,
        peer_id: PeerId,
        log_msg: GatewayLogMsg,
    ) -> Option<LogsCollectorEvent> {
        log::debug!("Got gateway log from {peer_id}: {log_msg:?}");
        let gateway_id = peer_id.to_base58();
        match log_msg.msg {
            Some(gateway_log_msg::Msg::QueryFinished(log)) if log.client_id == gateway_id => {
                Some(LogsCollectorEvent::QueryFinished(log))
            }
            Some(gateway_log_msg::Msg::QuerySubmitted(log)) if log.client_id == gateway_id => {
                Some(LogsCollectorEvent::QuerySubmitted(log))
            }
            _ => {
                log::warn!("Invalid gateway log messsage: {log_msg:?}");
                None
            }
        }
    }

    pub fn logs_collected(&mut self, logs_collected: LogsCollected) {
        self.inner.base.publish_logs_collected(logs_collected)
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
            InnerBehaviourEvent::Base(ev) => self.on_base_event(ev),
            InnerBehaviourEvent::WorkerLogs(Request {
                peer_id,
                request,
                response_channel,
            }) => {
                _ = self.inner.worker_logs.try_send_response(response_channel, 1);
                self.on_worker_logs(peer_id, request)
            }
            InnerBehaviourEvent::GatewayLogs(Request {
                peer_id,
                request,
                response_channel,
            }) => {
                _ = self.inner.gateway_logs.try_send_response(response_channel, 1);
                self.on_gateway_log(peer_id, request)
            }
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct LogsCollectorTransport {
    swarm: Swarm<Wrapped<LogsCollectorBehaviour>>,
    logs_collected_rx: mpsc::Receiver<LogsCollected>,
    events_tx: mpsc::Sender<LogsCollectorEvent>,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
}

impl LogsCollectorTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting logs collector P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(logs_collected) = self.logs_collected_rx.recv() => self.swarm.behaviour_mut().logs_collected(logs_collected),
            }
        }
        log::info!("Shutting down logs collector P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<LogsCollectorEvent>) {
        #[cfg(feature = "metrics")]
        self.metrics.record(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.try_send(ev).unwrap_or_else(|e| {
                log::error!("Logs collector event queue full. Event dropped: {e:?}")
            })
        }
    }
}

#[derive(Clone)]
pub struct LogsCollectorTransportHandle {
    logs_collected_tx: mpsc::Sender<LogsCollected>,
    _task_manager: Arc<TaskManager>,
}

impl LogsCollectorTransportHandle {
    fn new(
        logs_collected_tx: mpsc::Sender<LogsCollected>,
        transport: LogsCollectorTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            logs_collected_tx,
            _task_manager: Arc::new(task_manager),
        }
    }
    pub fn logs_collected(&self, logs_collected: LogsCollected) -> Result<(), QueueFull> {
        log::debug!("Queueing LogsCollected message: {logs_collected:?}");
        Ok(self.logs_collected_tx.try_send(logs_collected)?)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<LogsCollectorBehaviour>>,
    config: LogsCollectorConfig,
    #[cfg(feature = "metrics")] registry: &mut Registry,
) -> (impl Stream<Item = LogsCollectorEvent>, LogsCollectorTransportHandle) {
    let (logs_collected_tx, logs_collected_rx) = mpsc::channel(config.logs_collected_queue_size);
    let (events_tx, events_rx) = mpsc::channel(config.events_queue_size);
    let transport = LogsCollectorTransport {
        swarm,
        logs_collected_rx,
        events_tx,
        #[cfg(feature = "metrics")]
        metrics: Metrics::new(registry),
    };
    let handle =
        LogsCollectorTransportHandle::new(logs_collected_tx, transport, config.shutdown_timeout);
    (ReceiverStream::new(events_rx), handle)
}
