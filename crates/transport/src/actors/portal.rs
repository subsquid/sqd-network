use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Error;
use futures::{future::join_all, AsyncWriteExt, StreamExt};
use futures_core::Stream;
use libp2p::{
    kad::{GetProvidersError, GetProvidersOk, ProgressStep, QueryId, QueryStats, RecordKey},
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use prost::Message;
use serde::{Deserialize, Serialize};

use parking_lot::Mutex;
use sqd_messages::{Heartbeat, Query, QueryFinished, QueryResult};
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{
        MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, PORTAL_LOGS_PROTOCOL, PORTAL_LOGS_PROVIDER_KEY,
        QUERY_PROTOCOL,
    },
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortalEvent {
    Heartbeat {
        peer_id: PeerId,
        heartbeat: Heartbeat,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum LogListenersEvent {
    LogListeners { listeners: Vec<PeerId> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternalPortalEvent {
    Heartbeat {
        peer_id: PeerId,
        heartbeat: Heartbeat,
    },
    LogListeners {
        listeners: Vec<PeerId>,
    },
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
    pub events_queue_size: usize,
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
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            portal_logs_collector_lookup_interval: Duration::from_secs(60),
            log_sending_timeout: Duration::from_secs(10),
        }
    }
}

pub struct PortalTransport {
    swarm: Swarm<Wrapped<PortalBehaviour>>,
    events_tx: Sender<PortalEvent>,
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
                InternalPortalEvent::Heartbeat { peer_id, heartbeat } => {
                    self.events_tx.send_lossy(PortalEvent::Heartbeat { peer_id, heartbeat })
                }
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

    async fn send_log_to_listener(&self, listener: PeerId, buf: &[u8]) -> Result<(), Error> {
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

    async fn publish_portal_log(&self, log: &QueryFinished, listeners: &[PeerId]) {
        log::debug!("Sending log: {log:?}");
        let buf = log.encode_to_vec();
        let results =
            join_all(listeners.iter().map(|listener| self.send_log_to_listener(*listener, &buf)))
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

    pub async fn send_log(&self, log: &QueryFinished) {
        let listeners = self.log_listeners.lock().clone();
        self.publish_portal_log(log, &listeners).await;
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PortalBehaviour>>,
    config: PortalConfig,
) -> (impl Stream<Item = PortalEvent>, PortalTransportHandle) {
    let query_handle = swarm.behaviour().base.request_handle(QUERY_PROTOCOL, config.query_config);
    let logs_handle =
        swarm.behaviour().base.request_handle(PORTAL_LOGS_PROTOCOL, config.query_config);
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let (log_listeners_tx, log_listeners_rx) = new_queue(config.events_queue_size, "listeners");

    let transport = PortalTransport {
        swarm,
        events_tx,
        log_listeners_tx,
    };
    let handle = PortalTransportHandle::new(
        log_listeners_rx,
        logs_handle,
        query_handle,
        transport,
        config.shutdown_timeout,
        config.portal_logs_collector_lookup_interval,
        config.log_sending_timeout,
    );
    (events_rx, handle)
}

pub struct PortalBehaviour {
    base: Wrapped<BaseBehaviour>,
    provider_query: Option<QueryId>,
    providers_list: HashSet<PeerId>,
}

impl PortalBehaviour {
    pub fn new(mut base: BaseBehaviour, _config: PortalConfig) -> Wrapped<Self> {
        base.start_pulling_heartbeats();

        Self {
            base: base.into(),
            provider_query: Default::default(),
            providers_list: Default::default(),
        }
        .into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<InternalPortalEvent> {
        match ev {
            BaseBehaviourEvent::Heartbeat { peer_id, heartbeat } => {
                log::debug!("Got heartbeat from {peer_id}");
                log::trace!("{heartbeat:?}");
                Some(InternalPortalEvent::Heartbeat { peer_id, heartbeat })
            }
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

impl From<RequestError> for QueryFailure {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Timeout(t) => Self::Timeout(t),
            e => Self::TransportError(e.to_string()),
        }
    }
}
