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

use sqd_messages::{LogsRequest, Query, QueryLogs, QueryResult, WorkerStatus};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        request_server::{Request, ServerBehaviour},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    protocol::{
        MAX_HEARTBEAT_SIZE, MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, MAX_QUERY_MSG_SIZE,
        MAX_QUERY_RESULT_SIZE, QUERY_PROTOCOL, WORKER_LOGS_PROTOCOL, WORKER_STATUS_PROTOCOL,
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
    StatusRequest {
        peer_id: PeerId,
        /// If this channel is dropped, the connection will be closed
        resp_chan: ResponseChannel<WorkerStatus>,
    },
}

type QueryBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Query, QueryResult>>>;
type LogsBehaviour = Wrapped<ServerBehaviour<ProtoCodec<LogsRequest, QueryLogs>>>;
type StatusBehaviour = Wrapped<ServerBehaviour<ProtoCodec<(), WorkerStatus>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    query: QueryBehaviour,
    logs: LogsBehaviour,
    status: StatusBehaviour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub heartbeats_queue_size: usize,
    pub query_results_queue_size: usize,
    pub logs_queue_size: usize,
    pub status_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub send_logs_timeout: Duration,
}

impl WorkerConfig {
    pub fn new() -> Self {
        Self {
            heartbeats_queue_size: 100,
            query_results_queue_size: 100,
            logs_queue_size: 1,
            status_queue_size: 10,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            query_execution_timeout: Duration::from_secs(20),
            send_logs_timeout: Duration::from_secs(5),
        }
    }
}

pub struct WorkerBehaviour {
    inner: InnerBehaviour,
}

impl WorkerBehaviour {
    pub fn new(mut base: BaseBehaviour, config: WorkerConfig) -> Wrapped<Self> {
        base.set_server_mode();
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                query: ServerBehaviour::new(
                    ProtoCodec::new(MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE),
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
                status: ServerBehaviour::new(
                    ProtoCodec::new(0, MAX_HEARTBEAT_SIZE),
                    WORKER_STATUS_PROTOCOL,
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

    fn on_status_request(
        &mut self,
        peer_id: PeerId,
        _request: (),
        resp_chan: ResponseChannel<WorkerStatus>,
    ) -> Option<WorkerEvent> {
        log::debug!("Status requested by {peer_id}");
        Some(WorkerEvent::StatusRequest { peer_id, resp_chan })
    }

    pub fn send_logs(&mut self, logs: QueryLogs, resp_chan: ResponseChannel<QueryLogs>) {
        log::debug!("Sending {} query logs", logs.queries_executed.len());

        self.inner
            .logs
            .try_send_response(resp_chan, logs)
            .unwrap_or_else(|_| log::error!("Couldn't send logs"));
    }

    pub fn send_status(&mut self, status: WorkerStatus, resp_chan: ResponseChannel<WorkerStatus>) {
        log::debug!("Sending status on request");

        self.inner
            .status
            .try_send_response(resp_chan, status)
            .unwrap_or_else(|_| log::debug!("Couldn't send status"));
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
            InnerBehaviourEvent::Status(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_status_request(peer_id, request, response_channel),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct WorkerTransport {
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    query_results_rx: Receiver<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_rx: Receiver<(QueryLogs, ResponseChannel<QueryLogs>)>,
    status_rx: Receiver<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
    events_tx: Sender<WorkerEvent>,
}

impl WorkerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        loop {
            tokio::select! {
                 _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some((res, resp_chan)) = self.query_results_rx.recv() => self.swarm.behaviour_mut().send_query_result(res, resp_chan),
                Some((logs, resp_chan)) = self.logs_rx.recv() => self.swarm.behaviour_mut().send_logs(logs, resp_chan),
                Some((status, resp_chan)) = self.status_rx.recv() => self.swarm.behaviour_mut().send_status(status, resp_chan),
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
    query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
    status_tx: Sender<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(
        query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
        logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
        status_tx: Sender<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
        transport: WorkerTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            query_results_tx,
            logs_tx,
            status_tx,
            _task_manager: Arc::new(task_manager),
        }
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

    pub fn send_status(
        &self,
        status: WorkerStatus,
        resp_chan: ResponseChannel<WorkerStatus>,
    ) -> Result<(), QueueFull> {
        log::debug!("Queueing worker status");
        self.status_tx.try_send((status, resp_chan))
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    config: WorkerConfig,
) -> (impl Stream<Item = WorkerEvent>, WorkerTransportHandle) {
    let (query_results_tx, query_results_rx) =
        new_queue(config.query_results_queue_size, "query_results");
    let (logs_tx, logs_rx) = new_queue(config.logs_queue_size, "logs");
    let (status_tx, status_rx) = new_queue(config.status_queue_size, "status");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = WorkerTransport {
        swarm,
        query_results_rx,
        logs_rx,
        status_rx,
        events_tx,
    };
    let handle = WorkerTransportHandle::new(
        query_results_tx,
        logs_tx,
        status_tx,
        transport,
        config.shutdown_timeout,
    );
    (events_rx, handle)
}
