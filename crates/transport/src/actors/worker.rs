use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    request_response::ResponseChannel,
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
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

#[derive(Debug)]
pub enum WorkerEvent {
    /// Query received from a gateway
    Query {
        peer_id: PeerId,
        query: Query,
        /// If this channel is dropped, the connection will be closed
        resp_chan: ResponseChannel<QueryResult>,
    },
    /// Logs requested by a collector
    LogsRequest {
        request: LogsRequest,
        /// If this channel is dropped, the connection will be closed
        resp_chan: ResponseChannel<QueryLogs>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub heartbeats_queue_size: usize,
    pub query_results_queue_size: usize,
    pub logs_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub send_logs_timeout: Duration,
    pub service_nodes: Vec<PeerId>,
}

impl WorkerConfig {
    pub fn new() -> Self {
        Self {
            heartbeats_queue_size: 100,
            query_results_queue_size: 100,
            logs_queue_size: 1,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            query_execution_timeout: Duration::from_secs(20),
            send_logs_timeout: Duration::from_secs(5),
            service_nodes: Default::default(),
        }
    }
}

pub struct WorkerBehaviour {
    inner: InnerBehaviour,
}

impl WorkerBehaviour {
    pub fn new(mut base: BaseBehaviour, config: WorkerConfig) -> Wrapped<Self> {
        base.subscribe_heartbeats();
        for peer in config.service_nodes {
            base.allow_peer(peer);
        }
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
        Some(WorkerEvent::Query {
            peer_id,
            query,
            resp_chan,
        })
    }

    pub fn send_heartbeat(&mut self, heartbeat: Heartbeat) {
        self.inner.base.publish_heartbeat(heartbeat);
    }

    pub fn send_query_result(
        &mut self,
        result: QueryResult,
        resp_chan: ResponseChannel<QueryResult>,
    ) {
        log::debug!("Sending query result {result:?}");

        self.inner
            .query
            .try_send_response(resp_chan, result)
            .unwrap_or_else(|e| log::error!("Cannot send result for query {}", e.query_id));
    }

    fn on_logs_request(
        &mut self,
        _peer_id: PeerId,
        request: LogsRequest,
        resp_chan: ResponseChannel<QueryLogs>,
    ) -> Option<WorkerEvent> {
        Some(WorkerEvent::LogsRequest { request, resp_chan })
    }

    pub fn send_logs(&mut self, logs: QueryLogs, resp_chan: ResponseChannel<QueryLogs>) {
        log::debug!("Sending {} query logs", logs.queries_executed.len());

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
    heartbeats_rx: Receiver<Heartbeat>,
    query_results_rx: Receiver<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_rx: Receiver<(QueryLogs, ResponseChannel<QueryLogs>)>,
    events_tx: Sender<WorkerEvent>,
}

impl WorkerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        loop {
            tokio::select! {
                 _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(heartbeat) = self.heartbeats_rx.recv() => self.swarm.behaviour_mut().send_heartbeat(heartbeat),
                Some((res, resp_chan)) = self.query_results_rx.recv() => self.swarm.behaviour_mut().send_query_result(res, resp_chan),
                Some((logs, resp_chan)) = self.logs_rx.recv() => self.swarm.behaviour_mut().send_logs(logs, resp_chan),
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
    heartbeats_tx: Sender<Heartbeat>,
    query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(
        heartbeats_tx: Sender<Heartbeat>,
        query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
        logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
        transport: WorkerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            heartbeats_tx,
            query_results_tx,
            logs_tx,
            _task_manager: Arc::new(task_manager),
        }
    }

    pub fn send_heartbeat(&self, heartbeat: Heartbeat) -> Result<(), QueueFull> {
        log::trace!("Queueing heartbeat {heartbeat:?}");
        self.heartbeats_tx.try_send(heartbeat)
    }

    pub fn send_query_result(
        &self,
        result: QueryResult,
        resp_chan: ResponseChannel<QueryResult>,
    ) -> Result<(), QueueFull> {
        log::debug!("Queueing query result {result:?}");
        self.query_results_tx.try_send((result, resp_chan))
    }

    pub fn send_logs(
        &self,
        logs: QueryLogs,
        resp_chan: ResponseChannel<QueryLogs>,
    ) -> Result<(), QueueFull> {
        log::debug!("Queueing {} query logs", logs.queries_executed.len());
        self.logs_tx.try_send((logs, resp_chan))
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    config: WorkerConfig,
) -> (impl Stream<Item = WorkerEvent>, WorkerTransportHandle) {
    let (heartbeats_tx, heartbeats_rx) = new_queue(config.heartbeats_queue_size, "heartbeats");
    let (query_results_tx, query_results_rx) =
        new_queue(config.query_results_queue_size, "query_results");
    let (logs_tx, logs_rx) = new_queue(config.logs_queue_size, "logs");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = WorkerTransport {
        swarm,
        heartbeats_rx,
        query_results_rx,
        logs_rx,
        events_tx,
    };
    let handle = WorkerTransportHandle::new(
        heartbeats_tx,
        query_results_tx,
        logs_tx,
        transport,
        config.shutdown_timeout,
    );
    (events_rx, handle)
}
