use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    request_response::ResponseChannel,
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::{
    query_result, signatures::SignedMessage, LogsCollected, Ping, Pong, Query, QueryExecuted,
    QueryResult,
};

#[cfg(feature = "metrics")]
use crate::metrics::PONGS_RECEIVED;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        request_server::{Request, ServerBehaviour},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::{ProtoCodec, ACK_SIZE},
    protocol::{
        MAX_PONG_SIZE, MAX_QUERY_RESULT_SIZE, MAX_QUERY_SIZE, PONG_PROTOCOL, QUERY_PROTOCOL,
    },
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerEvent {
    /// Pong message received from the scheduler
    Pong(Pong),
    /// Query received from a gateway
    Query { peer_id: PeerId, query: Query },
    /// Logs up to `last_seq_no` have been saved by logs collector
    LogsCollected { last_seq_no: Option<u64> },
}

type PongBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Pong, u32>>>;
type QueryBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Query, QueryResult>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    pong: PongBehaviour,
    query: QueryBehaviour,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub scheduler_id: PeerId,
    pub logs_collector_id: PeerId,
    pub max_pong_size: u64,
    pub max_query_size: u64,
    pub max_query_result_size: u64,
    pub pings_queue_size: usize,
    pub query_results_queue_size: usize,
    pub logs_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub pong_ack_timeout: Duration,
}

impl WorkerConfig {
    pub fn new(scheduler_id: PeerId, logs_collector_id: PeerId) -> Self {
        Self {
            scheduler_id,
            logs_collector_id,
            max_pong_size: MAX_PONG_SIZE,
            max_query_size: MAX_QUERY_SIZE,
            max_query_result_size: MAX_QUERY_RESULT_SIZE,
            pings_queue_size: 100,
            query_results_queue_size: 100,
            logs_queue_size: 100,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            query_execution_timeout: Duration::from_secs(20),
            pong_ack_timeout: Duration::from_secs(5),
        }
    }
}

pub struct WorkerBehaviour {
    inner: InnerBehaviour,
    local_peer_id: String,
    scheduler_id: PeerId,
    query_response_channels: HashMap<String, ResponseChannel<QueryResult>>,
}

impl WorkerBehaviour {
    pub fn new(
        mut base: BaseBehaviour,
        local_peer_id: PeerId,
        config: WorkerConfig,
    ) -> Wrapped<Self> {
        base.subscribe_pings();
        base.subscribe_worker_logs(config.logs_collector_id);
        base.allow_peer(config.logs_collector_id);
        base.allow_peer(config.scheduler_id);
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                pong: ServerBehaviour::new(
                    ProtoCodec::new(config.max_pong_size, ACK_SIZE),
                    PONG_PROTOCOL,
                    config.pong_ack_timeout,
                )
                .into(),
                query: ServerBehaviour::new(
                    ProtoCodec::new(config.max_query_size, config.max_query_result_size),
                    QUERY_PROTOCOL,
                    config.query_execution_timeout,
                )
                .into(),
            },
            local_peer_id: local_peer_id.to_base58(),
            scheduler_id: config.scheduler_id,
            query_response_channels: Default::default(),
        }
        .into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<WorkerEvent> {
        match ev {
            BaseBehaviourEvent::LogsCollected(logs_collected) => {
                self.on_logs_collected(logs_collected)
            }
            _ => None,
        }
    }

    fn on_logs_collected(&mut self, mut logs_collected: LogsCollected) -> Option<WorkerEvent> {
        log::debug!("Received logs collected message");
        // Extract last_seq_no for the local worker
        let last_seq_no = logs_collected.sequence_numbers.remove(&self.local_peer_id);
        Some(WorkerEvent::LogsCollected { last_seq_no })
    }

    fn on_query(
        &mut self,
        peer_id: PeerId,
        mut query: Query,
        resp_chan: Option<ResponseChannel<QueryResult>>,
    ) -> Option<WorkerEvent> {
        // Verify query signature
        if !query.verify_signature(&peer_id) {
            log::warn!("Dropping query with invalid signature from {peer_id}");
            return None;
        }
        // Check if query has ID
        let query_id = match &query.query_id {
            Some(id) => id.clone(),
            None => {
                log::warn!("Dropping query without ID from {peer_id}");
                return None;
            }
        };
        log::debug!("Query {query_id} verified");
        if let Some(resp_chan) = resp_chan {
            self.query_response_channels.insert(query_id, resp_chan);
        }
        Some(WorkerEvent::Query { peer_id, query })
    }

    fn on_pong_event(
        &mut self,
        Request {
            peer_id,
            request,
            response_channel,
        }: Request<Pong, u32>,
    ) -> Option<WorkerEvent> {
        if peer_id != self.scheduler_id {
            log::warn!("Peer {peer_id} impersonating scheduler");
            return None;
        }
        log::debug!("Received pong from scheduler: {request:?}");
        #[cfg(feature = "metrics")]
        PONGS_RECEIVED.inc();
        // Send minimal response to avoid getting errors
        _ = self.inner.pong.try_send_response(response_channel, 1);
        Some(WorkerEvent::Pong(request))
    }

    pub fn send_ping(&mut self, ping: Ping) {
        self.inner.base.publish_ping(ping);
    }

    pub fn send_query_result(&mut self, mut result: QueryResult) {
        log::debug!("Sending query result {result:?}");
        let Some(resp_chan) = self.query_response_channels.remove(&result.query_id) else {
            return log::error!("No response channel for query: {}", result.query_id);
        };

        // Check query result size limit
        let result_size = result.encoded_len() as u64;
        if result_size > MAX_QUERY_RESULT_SIZE {
            let err = format!("Query result size too large: {result_size}");
            log::error!("{err}");
            result = QueryResult {
                query_id: result.query_id,
                result: Some(query_result::Result::ServerError(err)),
            };
        }

        self.inner
            .query
            .try_send_response(resp_chan, result)
            .unwrap_or_else(|e| log::error!("Cannot send result for query {}", e.query_id));
    }

    pub fn send_logs(&mut self, logs: Vec<QueryExecuted>) {
        log::debug!("Sending query logs");
        self.inner.base.publish_worker_logs(logs);
    }
}

impl BehaviourWrapper for WorkerBehaviour {
    type Inner = InnerBehaviour;
    type Event = WorkerEvent;

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
            InnerBehaviourEvent::Query(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_query(peer_id, request, Some(response_channel)),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct WorkerTransport {
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    pings_rx: Receiver<Ping>,
    query_results_rx: Receiver<QueryResult>,
    logs_rx: Receiver<Vec<QueryExecuted>>,
    events_tx: Sender<WorkerEvent>,
}

impl WorkerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        loop {
            tokio::select! {
                 _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(ping) = self.pings_rx.recv() => self.swarm.behaviour_mut().send_ping(ping),
                Some(res) = self.query_results_rx.recv() => self.swarm.behaviour_mut().send_query_result(res),
                Some(logs) = self.logs_rx.recv() => self.swarm.behaviour_mut().send_logs(logs),
            }
        }
        log::info!("Shutting down worker P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<WorkerEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct WorkerTransportHandle {
    pings_tx: Sender<Ping>,
    query_results_tx: Sender<QueryResult>,
    logs_tx: Sender<Vec<QueryExecuted>>,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(
        pings_tx: Sender<Ping>,
        query_results_tx: Sender<QueryResult>,
        logs_tx: Sender<Vec<QueryExecuted>>,
        transport: WorkerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            pings_tx,
            query_results_tx,
            logs_tx,
            _task_manager: Arc::new(task_manager),
        }
    }

    pub fn send_ping(&self, ping: Ping) -> Result<(), QueueFull> {
        log::trace!("Queueing ping {ping:?}");
        self.pings_tx.try_send(ping)
    }

    pub fn send_query_result(&self, result: QueryResult) -> Result<(), QueueFull> {
        log::debug!("Queueing query result {result:?}");
        self.query_results_tx.try_send(result)
    }

    pub fn send_logs(&self, logs: Vec<QueryExecuted>) -> Result<(), QueueFull> {
        log::debug!("Queueing {} query logs", logs.len());
        self.logs_tx.try_send(logs)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    config: WorkerConfig,
) -> (impl Stream<Item = WorkerEvent>, WorkerTransportHandle) {
    let (pings_tx, pings_rx) = new_queue(config.pings_queue_size, "pings");
    let (query_results_tx, query_results_rx) =
        new_queue(config.query_results_queue_size, "query_results");
    let (logs_tx, logs_rx) = new_queue(config.logs_queue_size, "logs");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = WorkerTransport {
        swarm,
        pings_rx,
        query_results_rx,
        logs_rx,
        events_tx,
    };
    let handle = WorkerTransportHandle::new(
        pings_tx,
        query_results_tx,
        logs_tx,
        transport,
        config.shutdown_timeout,
    );
    (events_rx, handle)
}
