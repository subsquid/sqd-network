use std::{sync::Arc, time::Duration};

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

use sqd_messages::{FileError, FileRequest, FileResponse, LogsRequest, Query, QueryLogs, QueryResult, WorkerStatus, file_error, file_response};

use crate::{
    ClientConfig, QueueFull, behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent}, noise::NoiseBehaviour, request_server::{Request, ServerBehaviour}, stream_client::{StreamClientHandle, Timeout, RequestError}, wrapped::{BehaviourWrapper, TToSwarm, Wrapped}
    }, codec::ProtoCodec, protocol::{
        FILE_PROTOCOL, MAX_FILE_REQUEST_SIZE, MAX_FILE_RESPONSE_SIZE, MAX_HEARTBEAT_SIZE, MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, MAX_SQL_QUERY_MSG_SIZE, QUERY_PROTOCOL, SQL_QUERY_PROTOCOL, WORKER_LOGS_PROTOCOL, WORKER_STATUS_PROTOCOL
    }, record_event, util::{DEFAULT_SHUTDOWN_TIMEOUT, Receiver, Sender, TaskManager, new_queue}
};

#[derive(Debug)]
pub enum WorkerEvent {
    /// Query received from a portal
    Query {
        peer_id: PeerId,
        query: Query,
        /// If this channel is dropped, the connection will be closed
        resp_chan: ResponseChannel<QueryResult>,
    },
    /// SQLQuery received from a portal
    SqlQuery {
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
    FileRequest {
        request: FileRequest,
        /// If this channel is dropped, the connection will be closed
        resp_chan: ResponseChannel<FileResponse>,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRequestError {
    InvalidRequest(String),
    Timeout(Timeout),
    ExecutionError(file_error::Err),
    TransportError(String),
    InvalidResponse(String),
}

type QueryBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Query, QueryResult>>>;
type SqlQueryBehaviour = Wrapped<ServerBehaviour<ProtoCodec<Query, QueryResult>>>;
type LogsBehaviour = Wrapped<ServerBehaviour<ProtoCodec<LogsRequest, QueryLogs>>>;
type StatusBehaviour = Wrapped<ServerBehaviour<ProtoCodec<(), WorkerStatus>>>;
type FileBehaviour = Wrapped<ServerBehaviour<ProtoCodec<FileRequest, FileResponse>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    query: QueryBehaviour,
    sql_query: SqlQueryBehaviour,
    logs: LogsBehaviour,
    status: StatusBehaviour,
    noise: NoiseBehaviour,
    file: FileBehaviour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub heartbeats_queue_size: usize,
    pub query_results_queue_size: usize,
    pub sql_query_results_queue_size: usize,
    pub logs_queue_size: usize,
    pub status_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub query_execution_timeout: Duration,
    pub send_logs_timeout: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            heartbeats_queue_size: 100,
            query_results_queue_size: 100,
            sql_query_results_queue_size: 100,
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
    pub fn new(mut base: BaseBehaviour, config: &WorkerConfig) -> Wrapped<Self> {
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
                sql_query: ServerBehaviour::new(
                    ProtoCodec::new(MAX_SQL_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE),
                    SQL_QUERY_PROTOCOL,
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
                noise: NoiseBehaviour::default(),
                file: ServerBehaviour::new(
                    ProtoCodec::new(MAX_FILE_REQUEST_SIZE, MAX_FILE_RESPONSE_SIZE),
                    FILE_PROTOCOL,
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
        // Drop empty messages
        if query == Query::default() {
            None
        } else {
            Some(WorkerEvent::Query {
                peer_id,
                query,
                resp_chan,
            })
        }
    }

    fn on_sql_query(
        &mut self,
        peer_id: PeerId,
        query: Query,
        resp_chan: ResponseChannel<QueryResult>,
    ) -> Option<WorkerEvent> {
        // Drop empty messages
        if query == Query::default() {
            None
        } else {
            Some(WorkerEvent::SqlQuery {
                peer_id,
                query,
                resp_chan,
            })
        }
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

    pub fn send_sql_query_result(
        &mut self,
        result: QueryResult,
        resp_chan: ResponseChannel<QueryResult>,
    ) {
        log::debug!("Sending sql query result {result:?}");

        self.inner
            .sql_query
            .try_send_response(resp_chan, result)
            .unwrap_or_else(|e| log::error!("Cannot send sql result for query {}", e.query_id));
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

    fn on_file_request(
        &mut self,
        peer_id: PeerId,
        request: FileRequest,
        resp_chan: ResponseChannel<FileResponse>,
    ) -> Option<WorkerEvent> {
        log::info!("File requested by {peer_id}: {:?} {:?}..{:?}", request.file_id, request.offset, request.offset + request.len);
        Some(WorkerEvent::FileRequest { request, resp_chan })
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

    pub fn send_file(&mut self, file_chunk: FileResponse, resp_chan: ResponseChannel<FileResponse>) {
        log::info!("Sending file");

        self.inner
            .file
            .try_send_response(resp_chan, file_chunk)
            .unwrap_or_else(|_| log::error!("Couldn't send file"));
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
            InnerBehaviourEvent::SqlQuery(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_sql_query(peer_id, request, response_channel),
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
            InnerBehaviourEvent::File(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_file_request(peer_id, request, response_channel),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct WorkerTransport {
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    query_results_rx: Receiver<(QueryResult, ResponseChannel<QueryResult>)>,
    sql_query_results_rx: Receiver<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_rx: Receiver<(QueryLogs, ResponseChannel<QueryLogs>)>,
    status_rx: Receiver<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
    file_rx: Receiver<(FileResponse, ResponseChannel<FileResponse>)>,
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
                Some((res, resp_chan)) = self.sql_query_results_rx.recv() => self.swarm.behaviour_mut().send_sql_query_result(res, resp_chan),
                Some((logs, resp_chan)) = self.logs_rx.recv() => self.swarm.behaviour_mut().send_logs(logs, resp_chan),
                Some((status, resp_chan)) = self.status_rx.recv() => self.swarm.behaviour_mut().send_status(status, resp_chan),
                Some((file_chunk, resp_chan)) = self.file_rx.recv() => self.swarm.behaviour_mut().send_file(file_chunk, resp_chan),
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
    sql_query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
    logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
    status_tx: Sender<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
    file_tx: Sender<(FileResponse, ResponseChannel<FileResponse>)>,
    file_handle: StreamClientHandle,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

impl WorkerTransportHandle {
    fn new(
        query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
        sql_query_results_tx: Sender<(QueryResult, ResponseChannel<QueryResult>)>,
        logs_tx: Sender<(QueryLogs, ResponseChannel<QueryLogs>)>,
        status_tx: Sender<(WorkerStatus, ResponseChannel<WorkerStatus>)>,
        file_tx: Sender<(FileResponse, ResponseChannel<FileResponse>)>,
        transport: WorkerTransport,
        shutdown_timeout: Duration,
        file_handle: StreamClientHandle,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            query_results_tx,
            sql_query_results_tx,
            logs_tx,
            status_tx,
            file_tx,
            file_handle,
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

    pub fn send_sql_query_result(
        &self,
        result: QueryResult,
        resp_chan: ResponseChannel<QueryResult>,
    ) -> Result<(), QueueFull> {
        log::debug!("Queueing sql query result {result:?}");
        self.sql_query_results_tx.try_send((result, resp_chan))
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

    pub fn send_file(
        &self,
        file_chunk: FileResponse,
        resp_chan: ResponseChannel<FileResponse>,
    ) -> Result<(), QueueFull> {
        log::debug!("Sending file chunk");
        self.file_tx.try_send((file_chunk, resp_chan))
    }

    /// Request a chunk of file from a specific peer
    pub async fn request_file(
        &self,
        file_id: String,
        offset: u64,
        len: u64,
        peer_id: PeerId,
    ) -> Result<Vec<u8>, FileRequestError> {
        log::debug!("Requesting file from {peer_id}");
        let request = FileRequest {
            file_id,
            offset,
            len
        };
        let request_data = request.encode_to_vec();
        if request_data.len() as u64 > MAX_FILE_REQUEST_SIZE {
            return Err(FileRequestError::InvalidRequest("Request too large".to_owned()));
        };

        let resp = self.file_handle.request_response(peer_id, b"").await.map_err(|e| {
            match e {
                RequestError::Timeout(e) => FileRequestError::Timeout(e),
                RequestError::UnsupportedProtocol => FileRequestError::TransportError("Unsupported Protocol".to_owned()),
                RequestError::ResponseTooLarge => FileRequestError::InvalidResponse("Response Too Large".to_owned()),
                RequestError::Io(e) => FileRequestError::TransportError(e.to_string()),
            }
        })?;
        let response = FileResponse::decode(resp.as_ref()).map_err(|e| {
            FileRequestError::InvalidResponse(format!("Error decoding file response: {e}"))
        })?;
        let response_data = match response.result.unwrap() {
            file_response::Result::Data(data) => data,
            file_response::Result::Error(error) => return match error.err {
                Some(error) => Err(FileRequestError::ExecutionError(error)),
                None => Err(FileRequestError::InvalidResponse("Unknown Error".to_owned()))
            }
        };
        log::debug!("Got file response from {peer_id}: {:?} bytes", response_data.len());
        Ok(response_data)
    }
    
}

pub fn start_transport(
    swarm: Swarm<Wrapped<WorkerBehaviour>>,
    config: &WorkerConfig,
) -> (impl Stream<Item = WorkerEvent>, WorkerTransportHandle) {
    let (query_results_tx, query_results_rx) =
        new_queue(config.query_results_queue_size, "query_results");
    let (sql_query_results_tx, sql_query_results_rx) =
        new_queue(config.sql_query_results_queue_size, "sql_query_results");
    let (logs_tx, logs_rx) = new_queue(config.logs_queue_size, "logs");
    let (status_tx, status_rx) = new_queue(config.status_queue_size, "status");
    let (file_tx, file_rx) = new_queue(config.status_queue_size, "file");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let file_handle = swarm.behaviour().inner.base.request_handle(FILE_PROTOCOL, ClientConfig {
        max_response_size: MAX_FILE_RESPONSE_SIZE,
        ..Default::default()
    });
    let transport = WorkerTransport {
        swarm,
        query_results_rx,
        sql_query_results_rx,
        logs_rx,
        status_rx,
        file_rx,
        events_tx,
    };
    let handle = WorkerTransportHandle::new(
        query_results_tx,
        sql_query_results_tx,
        logs_tx,
        status_tx,
        file_tx,
        transport,
        config.shutdown_timeout,
        file_handle,
    );
    (events_rx, handle)
}
