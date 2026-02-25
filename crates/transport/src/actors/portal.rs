use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Error;
use futures::{future::join_all, AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{
    kad::{GetProvidersError, GetProvidersOk, ProgressStep, QueryId, QueryStats, RecordKey},
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use prost::Message;
use serde::{Deserialize, Serialize};

use parking_lot::Mutex;
use sqd_messages::{PortalLogs, Query, QueryFinished, QueryResult};
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{
        MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, PORTAL_LOGS_PROTOCOL_V2,
        PORTAL_LOGS_PROVIDER_KEY, QUERY_PROTOCOL,
    },
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum LogListenersEvent {
    LogListeners { listeners: Vec<PeerId> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternalPortalEvent {
    LogListeners { listeners: Vec<PeerId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryFailure {
    InvalidRequest(String),
    Timeout(Timeout),
    TransportError(String),
    InvalidResponse(String),
}

#[derive(Debug, Clone, Copy)]
pub struct PortalConfig {
    pub query_config: ClientConfig,
    pub listeners_queue_size: usize,
    pub shutdown_timeout: Duration,
    pub portal_logs_collector_lookup_interval: Duration,
    pub log_sending_timeout: Duration,
}

impl Default for PortalConfig {
    fn default() -> Self {
        Self {
            query_config: ClientConfig {
                max_response_size: MAX_QUERY_RESULT_SIZE,
                ..Default::default()
            },
            listeners_queue_size: 20,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            portal_logs_collector_lookup_interval: Duration::from_secs(60),
            log_sending_timeout: Duration::from_secs(10),
        }
    }
}

pub struct PortalTransport {
    swarm: Swarm<Wrapped<PortalBehaviour>>,
    log_listeners_tx: Sender<LogListenersEvent>,
}

impl PortalTransport {
    pub async fn run(
        mut self,
        portal_logs_collector_lookup_interval: Duration,
        cancel_token: CancellationToken,
    ) {
        log::info!("Starting portal P2P transport");
        let mut interval = time::interval(portal_logs_collector_lookup_interval);
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = interval.tick() => {
                    self.swarm.behaviour_mut().update_portal_logs_listeners();
                    log::debug!("listeners requested");
                },
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down portal P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<InternalPortalEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            match ev {
                InternalPortalEvent::LogListeners { listeners } => {
                    self.log_listeners_tx.send_lossy(LogListenersEvent::LogListeners { listeners })
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct PortalTransportHandle {
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
    query_handle: StreamClientHandle,
    logs_handle: StreamClientHandle,
    log_listeners: Arc<Mutex<Vec<PeerId>>>,
    log_sending_timeout: Duration,
}

impl PortalTransportHandle {
    fn new(
        mut log_listeners_rx: Receiver<LogListenersEvent>,
        logs_handle: StreamClientHandle,
        query_handle: StreamClientHandle,
        transport: PortalTransport,
        shutdown_timeout: Duration,
        portal_logs_collector_lookup_interval: Duration,
        log_sending_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(portal_logs_collector_lookup_interval, c));

        let log_listeners: Arc<Mutex<Vec<PeerId>>> = Default::default();
        let local_listeners = log_listeners.clone();

        task_manager.spawn(|cancel_token| async move {
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    Some(LogListenersEvent::LogListeners { listeners }) = log_listeners_rx.next() => {
                        *local_listeners.lock() = listeners;
                    }
                }
            }
        });
        Self {
            _task_manager: Arc::new(task_manager),
            query_handle,
            logs_handle,
            log_listeners,
            log_sending_timeout,
        }
    }

    pub async fn send_query(
        &self,
        peer_id: PeerId,
        query: Query,
    ) -> Result<QueryResult, QueryFailure> {
        let query_size = query.encoded_len() as u64;
        if query_size > MAX_QUERY_MSG_SIZE {
            return Err(QueryFailure::InvalidRequest(format!(
                "Query size too large: {query_size}"
            )));
        }

        log::debug!("Sending query {query:?}");

        let buf = query.encode_to_vec();
        let resp_buf = self.query_handle.request_response(peer_id, &buf).await?;

        if resp_buf.is_empty() {
            // Empty response is a sign of worker error
            log::warn!("Empty response for query from peer {peer_id}");
            return Err(QueryFailure::InvalidResponse("Empty response".to_string()));
        }
        let result = QueryResult::decode(resp_buf.as_slice())
            .map_err(|e| QueryFailure::InvalidResponse(e.to_string()))?;

        log::debug!("Got query result from {peer_id}");
        Ok(result)
    }

    pub async fn send_query_request(
        &self,
        peer_id: PeerId,
        query: Query,
    ) -> Result<QueryResponseReader, QueryFailure> {
        let query_size = query.encoded_len() as u64;
        if query_size > MAX_QUERY_MSG_SIZE {
            return Err(QueryFailure::InvalidRequest(format!(
                "Query size too large: {query_size}"
            )));
        }

        let buf = query.encode_to_vec();
        let mut stream = self.query_handle.get_raw_stream(peer_id).await?;
        stream
            .write_all(&buf)
            .await
            .map_err(|e| QueryFailure::from(RequestError::from(e)))?;
        stream
            .close()
            .await
            .map_err(|e| QueryFailure::from(RequestError::from(e)))?;

        Ok(QueryResponseReader {
            stream: Box::new(stream),
            buf: Vec::with_capacity(1024 * 1024),
            max_response_size: self.query_handle.config.max_response_size,
            request_timeout: self.query_handle.config.request_timeout,
        })
    }

    async fn send_logs_to_listener(&self, listener: PeerId, buf: &[u8]) -> Result<(), Error> {
        let fut = async {
            let mut stream = self.logs_handle.get_raw_stream(listener).await?;
            stream.write_all(buf).await?;
            stream.close().await?;
            Ok(())
        };
        tokio::time::timeout(self.log_sending_timeout, fut).await.unwrap_or_else(|_| {
            log::debug!("Log sending to {listener} timed out");
            Err(RequestError::Timeout(Timeout::Connect))?
        })
    }

    async fn publish_portal_logs(&self, portal_logs: Vec<QueryFinished>, listeners: &[PeerId]) {
        log::trace!("Sending logs: {portal_logs:?}");
        let payload = PortalLogs { portal_logs };
        let buffer = payload.encode_to_vec();
        let results = join_all(
            listeners
                .iter()
                .map(|listener| self.send_logs_to_listener(*listener, buffer.as_ref())),
        )
        .await;
        for (idx, result) in results.iter().enumerate() {
            let listener = listeners[idx];
            match result {
                Ok(_) => {
                    log::trace!("Logs sent to {listener:?}");
                }
                Err(err) => {
                    log::warn!("Failed to send logs to {listener:?}: {err:?}");
                }
            }
        }
    }

    pub async fn send_logs(&self, logs: Vec<QueryFinished>) {
        let listeners = self.log_listeners.lock().clone();
        self.publish_portal_logs(logs, &listeners).await;
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PortalBehaviour>>,
    config: PortalConfig,
) -> PortalTransportHandle {
    let query_handle = swarm.behaviour().base.request_handle(QUERY_PROTOCOL, config.query_config);
    let logs_handle = swarm
        .behaviour()
        .base
        .request_handle(PORTAL_LOGS_PROTOCOL_V2, config.query_config);
    let (log_listeners_tx, log_listeners_rx) = new_queue(config.listeners_queue_size, "listeners");

    let transport = PortalTransport {
        swarm,
        log_listeners_tx,
    };
    PortalTransportHandle::new(
        log_listeners_rx,
        logs_handle,
        query_handle,
        transport,
        config.shutdown_timeout,
        config.portal_logs_collector_lookup_interval,
        config.log_sending_timeout,
    )
}

pub struct PortalBehaviour {
    base: Wrapped<BaseBehaviour>,
    provider_query: Option<QueryId>,
    providers_list: HashSet<PeerId>,
}

impl PortalBehaviour {
    pub fn new(mut base: BaseBehaviour, _config: PortalConfig) -> Wrapped<Self> {
        base.keep_all_connections_alive();
        Self {
            base: base.into(),
            provider_query: Default::default(),
            providers_list: Default::default(),
        }
        .into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<InternalPortalEvent> {
        match ev {
            BaseBehaviourEvent::ProviderRecord {
                id,
                result,
                stats,
                step,
            } => self.on_provider_record(id, result, &stats, &step),
        }
    }

    fn on_provider_record(
        &mut self,
        id: QueryId,
        result: Result<GetProvidersOk, GetProvidersError>,
        stats: &QueryStats,
        step: &ProgressStep,
    ) -> Option<InternalPortalEvent> {
        if let Some(expected_id) = self.provider_query {
            if expected_id != id {
                return None;
            }
        } else {
            return None;
        }
        log::debug!("Got provider record: {result:?} {stats:?} {step:?}");

        if let Ok(GetProvidersOk::FoundProviders { providers, .. }) = result {
            providers.iter().for_each(|p| {
                self.providers_list.insert(*p);
            });
        }
        if step.last {
            self.provider_query = None;
            return Some(InternalPortalEvent::LogListeners {
                listeners: self.providers_list.drain().collect(),
            });
        }
        None
    }

    pub fn update_portal_logs_listeners(&mut self) {
        let key = RecordKey::new(PORTAL_LOGS_PROVIDER_KEY);
        if self.provider_query.is_some() {
            return;
        }
        let query_id = self.base.get_kademlia_mut().get_providers(key);
        self.provider_query = Some(query_id);
    }
}

impl BehaviourWrapper for PortalBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = InternalPortalEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.base
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let out = self.on_base_event(ev);
        out.map(ToSwarm::GenerateEvent)
    }
}

pub struct QueryResponseReader {
    stream: Box<dyn futures::AsyncRead + Unpin + Send>,
    buf: Vec<u8>,
    max_response_size: u64,
    request_timeout: Duration,
}

impl QueryResponseReader {
    pub async fn wait_for_response(&mut self) -> Result<(), QueryFailure> {
        if !self.buf.is_empty() {
            return Ok(());
        }
        let fut = async {
            let mut byte = [0u8; 1];
            let n = self
                .stream
                .read(&mut byte)
                .await
                .map_err(|e| QueryFailure::TransportError(e.to_string()))?;
            if n == 0 {
                return Err(QueryFailure::InvalidResponse("Empty response".into()));
            }
            self.buf.push(byte[0]);
            Ok(())
        };
        tokio::time::timeout(self.request_timeout, fut)
            .await
            .unwrap_or_else(|_| Err(QueryFailure::Timeout(Timeout::Request)))
    }

    pub async fn read_response(mut self) -> Result<QueryResult, QueryFailure> {
        let remaining = self.max_response_size + 1 - self.buf.len() as u64;
        let fut = async {
            self.stream
                .take(remaining)
                .read_to_end(&mut self.buf)
                .await
                .map_err(|e| QueryFailure::TransportError(e.to_string()))?;
            if self.buf.len() as u64 > self.max_response_size {
                return Err(QueryFailure::TransportError("response too large".into()));
            }
            if self.buf.is_empty() {
                return Err(QueryFailure::InvalidResponse("Empty response".into()));
            }
            QueryResult::decode(self.buf.as_slice())
                .map_err(|e| QueryFailure::InvalidResponse(e.to_string()))
        };
        tokio::time::timeout(self.request_timeout, fut)
            .await
            .unwrap_or_else(|_| Err(QueryFailure::Timeout(Timeout::Request)))
    }


}

impl From<RequestError> for QueryFailure {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Timeout(t) => Self::Timeout(t),
            e => Self::TransportError(e.to_string()),
        }
    }
}
