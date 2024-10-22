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

use sqd_messages::{Heartbeat, LogsRequest, Query, QueryLogs, QueryResult};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        request_server::{Request, ServerBehaviour},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    protocol::{
        MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, MAX_QUERY_RESULT_SIZE, MAX_QUERY_SIZE,
        QUERY_PROTOCOL, WORKER_LOGS_PROTOCOL,
    },
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerEvent {
    /// Query received from a gateway
    Query { peer_id: PeerId, query: Query },
    /// Logs requested by a collector
    LogsRequest {
        peer_id: PeerId,
        request: LogsRequest,
    },
}

type QueryBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Query, QueryResult>>>;
type LogsBehaviour = Wrapped<ServerBehaviour<ProtoCodec<LogsRequest, QueryLogs>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    query: QueryBehaviour,
    logs: LogsBehaviour,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub pings_queue_size: usize,
    pub query_results_queue_size: usize,
    pub logs_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub send_logs_timeout: Duration,
}

impl WorkerConfig {
    pub fn new() -> Self {
        Self {
            pings_queue_size: 100,
            query_results_queue_size: 100,
            logs_queue_size: 1,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            query_execution_timeout: Duration::from_secs(20),
            send_logs_timeout: Duration::from_secs(5),
        }
    }
}

pub struct WorkerBehaviour {
    inner: InnerBehaviour,
    local_peer_id: PeerId,
    query_response_channels: HashMap<String, ResponseChannel<QueryResult>>,
    logs_response_channel: Option<ResponseChannel<QueryLogs>>,
}

impl WorkerBehaviour {
    pub fn new(
        mut base: BaseBehaviour,
        local_peer_id: PeerId,
        config: WorkerConfig,
    ) -> Wrapped<Self> {
        base.subscribe_pings();
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                query: ServerBehaviour::new(
                    ProtoCodec::new(MAX_QUERY_SIZE, MAX_QUERY_RESULT_SIZE),
                    QUERY_PROTOCOL,
                    config.query_execution_timeout,
                )
                .into(),
                logs: ServerBehaviour::new(
                    ProtoCodec::new(MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE),
                    WORKER_LOGS_PROTOCOL,
                    config.query_execution_timeout,
                )
                .into(),
            },
            local_peer_id,
            query_response_channels: Default::default(),
            logs_response_channel: None,
        }
        .into()
    }

    fn on_base_event(&mut self, _: BaseBehaviourEvent) -> Option<WorkerEvent> {
        None
    }

    fn on_query(
        &mut self,
        peer_id: PeerId,
        query: Query,
        resp_chan: ResponseChannel<QueryResult>,
    ) -> Option<WorkerEvent> {
        // Verify query signature
        if !query.verify_signature(peer_id, self.local_peer_id) {
            log::warn!("Dropping query with invalid signature from {peer_id}");
            return None;
        }
        // TODO: verify query timestamp

        // Check if query has ID
        let query_id = &query.query_id;
        if query_id.is_empty() {
            log::warn!("Dropping query without ID from {peer_id}");
            return None;
        };
        log::debug!("Query {query_id} verified");
        self.query_response_channels.insert(query_id.clone(), resp_chan);
        Some(WorkerEvent::Query { peer_id, query })
    }

    pub fn send_ping(&mut self, ping: Heartbeat) {
        self.inner.base.publish_ping(ping);
    }

    pub fn send_query_result(&mut self, result: QueryResult) {
        log::debug!("Sending query result {result:?}");
        let Some(resp_chan) = self.query_response_channels.remove(&result.query_id) else {
            panic!("No response channel for query: {}", result.query_id);
        };

        // Check query result size limit
        let result_size = result.encoded_len() as u64;
        if result_size > MAX_QUERY_RESULT_SIZE {
            log::error!("Query result size too large: {result_size}");
        }

        // TODO: sign the result

        self.inner
            .query
            .try_send_response(resp_chan, result)
            .unwrap_or_else(|e| log::error!("Cannot send result for query {}", e.query_id));
    }

    fn on_logs_request(
        &mut self,
        peer_id: PeerId,
        request: LogsRequest,
        resp_chan: ResponseChannel<QueryLogs>,
    ) -> Option<WorkerEvent> {
        if self.logs_response_channel.is_some() {
            log::warn!("Concurrent logs request from {peer_id}");
        }
        self.logs_response_channel = Some(resp_chan);
        Some(WorkerEvent::LogsRequest { peer_id, request })
    }

    pub fn send_logs(&mut self, logs: QueryLogs) {
        let Some(resp_chan) = self.logs_response_channel.take() else {
            log::warn!("No pending log requests");
            return;
        };
        log::debug!("Sending {} query logs", logs.queries_executed.len());

        // Check size limit
        let result_size = logs.encoded_len() as u64;
        if result_size > MAX_LOGS_RESPONSE_SIZE {
            log::error!("Logs size too large: {result_size}");
        }

        self.inner
            .logs
            .try_send_response(resp_chan, logs)
            .unwrap_or_else(|_| log::error!("Couldn't send logs"));
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
            InnerBehaviourEvent::Query(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_query(peer_id, request, response_channel),
            InnerBehaviourEvent::Logs(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_logs_request(peer_id, request, response_channel),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct WorkerTransport {
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    pings_rx: Receiver<Heartbeat>,
    query_results_rx: Receiver<QueryResult>,
    logs_rx: Receiver<QueryLogs>,
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
    pings_tx: Sender<Heartbeat>,
    query_results_tx: Sender<QueryResult>,
    logs_tx: Sender<QueryLogs>,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(
        pings_tx: Sender<Heartbeat>,
        query_results_tx: Sender<QueryResult>,
        logs_tx: Sender<QueryLogs>,
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

    pub fn send_ping(&self, ping: Heartbeat) -> Result<(), QueueFull> {
        log::trace!("Queueing ping {ping:?}");
        self.pings_tx.try_send(ping)
    }

    pub fn send_query_result(&self, result: QueryResult) -> Result<(), QueueFull> {
        log::debug!("Queueing query result {result:?}");
        self.query_results_tx.try_send(result)
    }

    pub fn send_logs(&self, logs: QueryLogs) -> Result<(), QueueFull> {
        log::debug!("Queueing {} query logs", logs.queries_executed.len());
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
