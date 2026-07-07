use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::{LogsRequest, Query, QueryLogs, QueryResult, WorkerStatus};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        noise::NoiseBehaviour,
        stream_server::{Request, ResponseError, ResponseSender, ServerBehaviour, ServerConfig},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{
        MAX_HEARTBEAT_SIZE, MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, MAX_QUERY_MSG_SIZE,
        MAX_QUERY_RESULT_SIZE, MAX_SQL_QUERY_MSG_SIZE, QUERY_PROTOCOL, SQL_QUERY_PROTOCOL,
        WORKER_LOGS_PROTOCOL, WORKER_STATUS_PROTOCOL,
    },
    record_event,
    util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug)]
pub enum WorkerEvent {
    /// Query received from a portal
    Query {
        peer_id: PeerId,
        query: Query,
        /// If this sender is dropped, the stream is reset and the client sees an error
        resp_chan: ResponseSender,
    },
    /// SQLQuery received from a portal
    SqlQuery {
        peer_id: PeerId,
        query: Query,
        /// If this sender is dropped, the stream is reset and the client sees an error
        resp_chan: ResponseSender,
    },
    /// Logs requested by a collector
    LogsRequest {
        request: LogsRequest,
        /// If this sender is dropped, the stream is reset and the client sees an error
        resp_chan: ResponseSender,
    },
    StatusRequest {
        peer_id: PeerId,
        /// If this sender is dropped, the stream is reset and the client sees an error
        resp_chan: ResponseSender,
    },
}

type QueryBehaviour = Wrapped<ServerBehaviour<Query>>;
type SqlQueryBehaviour = Wrapped<ServerBehaviour<Query>>;
type LogsBehaviour = Wrapped<ServerBehaviour<LogsRequest>>;
type StatusBehaviour = Wrapped<ServerBehaviour<()>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    query: QueryBehaviour,
    sql_query: SqlQueryBehaviour,
    logs: LogsBehaviour,
    status: StatusBehaviour,
    noise: NoiseBehaviour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub heartbeats_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub send_logs_timeout: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            heartbeats_queue_size: 100,
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
    pub fn new(mut base: BaseBehaviour, config: &WorkerConfig) -> Wrapped<Self> {
        base.set_server_mode();
        let server_config = |max_request_size: u64, max_response_size: u64| ServerConfig {
            max_request_size,
            max_response_size,
            read_timeout: config.query_execution_timeout,
            write_timeout: config.query_execution_timeout,
            ..Default::default()
        };
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                query: ServerBehaviour::new(
                    QUERY_PROTOCOL,
                    server_config(MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE),
                )
                .into(),
                sql_query: ServerBehaviour::new(
                    SQL_QUERY_PROTOCOL,
                    server_config(MAX_SQL_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE),
                )
                .into(),
                logs: ServerBehaviour::new(
                    WORKER_LOGS_PROTOCOL,
                    server_config(MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE),
                )
                .into(),
                status: ServerBehaviour::new(
                    WORKER_STATUS_PROTOCOL,
                    server_config(0, MAX_HEARTBEAT_SIZE),
                )
                .into(),
                noise: NoiseBehaviour::default(),
            },
        }
        .into()
    }

    fn on_base_event(&mut self, _: BaseBehaviourEvent) -> Option<WorkerEvent> {
        None
    }

    fn on_query(&mut self, req: Request<Query>) -> Option<WorkerEvent> {
        let Request {
            peer_id,
            request: query,
            response_sender,
        } = req;
        // Drop empty messages
        if query == Query::default() {
            None
        } else {
            Some(WorkerEvent::Query {
                peer_id,
                query,
                resp_chan: response_sender,
            })
        }
    }

    fn on_sql_query(&mut self, req: Request<Query>) -> Option<WorkerEvent> {
        let Request {
            peer_id,
            request: query,
            response_sender,
        } = req;
        // Drop empty messages
        if query == Query::default() {
            None
        } else {
            Some(WorkerEvent::SqlQuery {
                peer_id,
                query,
                resp_chan: response_sender,
            })
        }
    }

    fn on_logs_request(&mut self, req: Request<LogsRequest>) -> Option<WorkerEvent> {
        Some(WorkerEvent::LogsRequest {
            request: req.request,
            resp_chan: req.response_sender,
        })
    }

    fn on_status_request(&mut self, req: Request<()>) -> Option<WorkerEvent> {
        log::debug!("Status requested by {}", req.peer_id);
        Some(WorkerEvent::StatusRequest {
            peer_id: req.peer_id,
            resp_chan: req.response_sender,
        })
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
            InnerBehaviourEvent::Query(req) => self.on_query(req),
            InnerBehaviourEvent::SqlQuery(req) => self.on_sql_query(req),
            InnerBehaviourEvent::Logs(req) => self.on_logs_request(req),
            InnerBehaviourEvent::Status(req) => self.on_status_request(req),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct WorkerTransport {
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    events_tx: Sender<WorkerEvent>,
}

impl WorkerTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        loop {
            tokio::select! {
                 _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
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
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(transport: WorkerTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }

    pub async fn send_query_result(
        &self,
        result: QueryResult,
        resp_chan: ResponseSender,
    ) -> Result<(), ResponseError> {
        log::debug!("Sending query result {result:?}");
        resp_chan.send(&result.encode_to_vec()).await
    }

    pub async fn send_sql_query_result(
        &self,
        result: QueryResult,
        resp_chan: ResponseSender,
    ) -> Result<(), ResponseError> {
        log::debug!("Sending sql query result {result:?}");
        resp_chan.send(&result.encode_to_vec()).await
    }

    pub async fn send_logs(
        &self,
        logs: QueryLogs,
        resp_chan: ResponseSender,
    ) -> Result<(), ResponseError> {
        log::debug!("Sending {} query logs", logs.queries_executed.len());
        resp_chan.send(&logs.encode_to_vec()).await
    }

    pub async fn send_status(
        &self,
        status: WorkerStatus,
        resp_chan: ResponseSender,
    ) -> Result<(), ResponseError> {
        log::debug!("Sending worker status");
        resp_chan.send(&status.encode_to_vec()).await
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    config: &WorkerConfig,
) -> (impl Stream<Item = WorkerEvent>, WorkerTransportHandle) {
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = WorkerTransport { swarm, events_tx };
    let handle = WorkerTransportHandle::new(transport, config.shutdown_timeout);
    (events_rx, handle)
}
