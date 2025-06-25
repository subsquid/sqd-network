use std::{
    collections::HashSet, marker::PhantomData, sync::Arc, time::{Duration, Instant}
};

use anyhow::Error;
use futures::{AsyncWriteExt, StreamExt};
use futures_core::Stream;
use libp2p::{
    kad::{GetProvidersError, GetProvidersOk, ProgressStep, QueryId, QueryStats},
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use prost::Message;
use serde::{Deserialize, Serialize};

use sqd_messages::{Heartbeat, Query, QueryFinished, QueryResult};
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, PORTAL_LOGS_PROTOCOL, QUERY_PROTOCOL},
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayEvent {
    Heartbeat {
        peer_id: PeerId,
        heartbeat: Heartbeat,
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
pub struct GatewayConfig {
    pub query_config: ClientConfig,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    /// Subcribe to worker status via gossipsub (default: false).
    pub worker_status_via_gossipsub: bool,
    /// Subcribe to worker status via direct polling (default: true).
    pub worker_status_via_polling: bool,
    pub portal_logs_collector_lookup_time: Duration,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            query_config: ClientConfig {
                max_response_size: MAX_QUERY_RESULT_SIZE,
                ..Default::default()
            },
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            worker_status_via_gossipsub: false,
            worker_status_via_polling: true,
            portal_logs_collector_lookup_time: Duration::from_secs(60),
        }
    }
}

pub struct GatewayTransport {
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    logs_rx: Receiver<QueryFinished>,
    events_tx: Sender<GatewayEvent>,
    logs_handle: StreamClientHandle,
}

impl GatewayTransport {
    pub async fn run(mut self, portal_logs_collector_lookup_time: Duration, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        let mut interval = time::interval(portal_logs_collector_lookup_time);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.swarm.behaviour_mut().update_portal_logs_listeners();
                    log::debug!("listeners requested");
                },
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(log) = self.logs_rx.recv() => self.publish_portal_logs(log).await,
            }
        }
        log::info!("Shutting down worker P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<GatewayEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }

    async fn send_logs_to_listener(&mut self, listener: PeerId, buf: &Vec<u8>) -> Result<(), Error>{
        let mut stream = self.logs_handle.get_raw_stream(listener).await?;
        stream.write_all(&buf).await?;
        stream.close().await?;
        Ok(())
    }

    async fn publish_portal_logs(&mut self, log: QueryFinished) {
        log::debug!("Sending log: {log:?}");

        let buf = log.encode_to_vec();
        let listeners = self.swarm.behaviour().get_actual_portal_logs_listeners();
        let start = Instant::now();
        for listener in listeners {
            match self.send_logs_to_listener(listener, &buf).await {
                Ok(_) => {
                    log::trace!("Logs sent to {listener:?}");
                },
                Err(err) => {
                    log::error!("Failed to send logs to {listener:?}: {err:?}");
                },
            }
        }
        log::error!("Time to send: {:?}", start.elapsed());
    }
}

#[derive(Clone)]
pub struct GatewayTransportHandle {
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
    query_handle: StreamClientHandle,
    logs_tx: Sender<QueryFinished>,
}

impl GatewayTransportHandle {
    fn new(
        logs_tx: Sender<QueryFinished>,
        query_handle: StreamClientHandle,
        transport: GatewayTransport,
        shutdown_timeout: Duration,
        portal_logs_collector_lookup_time: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(portal_logs_collector_lookup_time, c));
        Self {
            _task_manager: Arc::new(task_manager),
            query_handle,
            logs_tx,
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

    pub fn send_logs(&self, log: QueryFinished) -> Result<(), QueueFull> {
        log::trace!("Queueing logs {log:?}");
        self.logs_tx.try_send(log)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    config: GatewayConfig,
) -> (impl Stream<Item = GatewayEvent>, GatewayTransportHandle) {
    let (logs_tx, logs_rx) = new_queue(config.events_queue_size, "portal_logs");

    let query_handle = swarm.behaviour().base.request_handle(QUERY_PROTOCOL, config.query_config);
    let logs_handle =
        swarm.behaviour().base.request_handle(PORTAL_LOGS_PROTOCOL, config.query_config);
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");

    let transport = GatewayTransport {
        swarm,
        logs_rx,
        events_tx,
        logs_handle,
    };
    let handle =
        GatewayTransportHandle::new(logs_tx, query_handle, transport, config.shutdown_timeout, config.portal_logs_collector_lookup_time);
    (events_rx, handle)
}

#[derive(Default)]
pub struct ProviderList {
    value: HashSet<PeerId>,
    pub roll: HashSet<PeerId>,
}

impl ProviderList {
    pub fn get(&self) -> Vec<PeerId> {
        self.value.iter().cloned().collect::<Vec<PeerId>>()
    }

    pub fn add(&mut self, peer_id: PeerId) {
        self.roll.insert(peer_id);
    }

    pub fn push(&mut self) {
        self.value = self.roll.drain().collect();
    }
}

pub struct GatewayBehaviour {
    base: Wrapped<BaseBehaviour>,
    provider_query: Option<QueryId>,
    providers: ProviderList,
}

impl GatewayBehaviour {
    pub fn new(mut base: BaseBehaviour, config: GatewayConfig) -> Wrapped<Self> {
        if config.worker_status_via_gossipsub {
            base.subscribe_heartbeats();
        }

        base.subscribe_portal_logs();
        base.update_portal_logs_listeners();
        base.start_pulling_heartbeats();

        Self {
            base: base.into(),
            provider_query: Default::default(),
            providers: Default::default(),
        }
        .into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<GatewayEvent> {
        match ev {
            BaseBehaviourEvent::Heartbeat { peer_id, heartbeat } => {
                self.on_heartbeat(peer_id, heartbeat)
            }
            BaseBehaviourEvent::ProviderRecord {
                id,
                result,
                stats,
                step,
            } => self.on_provider_record(id, result, stats, step),
            _ => None,
        }
    }

    fn on_provider_record(
        &mut self,
        id: QueryId,
        result: Result<GetProvidersOk, GetProvidersError>,
        stats: QueryStats,
        step: ProgressStep,
    ) -> Option<GatewayEvent> {
        if let Some(expected_id) = self.provider_query {
            if expected_id != id {
                return None;
            }
        } else {
            return None;
        }
        log::debug!("Got provider record: {result:?} {stats:?} {step:?}");

        match result {
            Ok(GetProvidersOk::FoundProviders { providers, .. }) => {
                providers.iter().for_each(|p| self.providers.add(p.clone()));
            },
            _ => {}
        }
        if step.last {
            self.provider_query = None;
            self.providers.push();
        }
        None
    }

    fn on_heartbeat(&mut self, peer_id: PeerId, heartbeat: Heartbeat) -> Option<GatewayEvent> {
        log::debug!("Got heartbeat from {peer_id}");
        log::trace!("{heartbeat:?}");
        Some(GatewayEvent::Heartbeat { peer_id, heartbeat })
    }

    pub fn update_portal_logs_listeners(&mut self) {
        let key = PORTAL_LOGS_PROTOCOL.to_owned();
        if self.provider_query.is_some() {
            return;
        }
        let query_id =
            self.base.get_kademlia_mut_ref().get_providers(key.as_bytes().to_vec().into());
        self.provider_query = Some(query_id);
    }

    pub fn get_actual_portal_logs_listeners(&self) -> Vec<PeerId> {
        self.providers.get()
    }
}

impl BehaviourWrapper for GatewayBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = GatewayEvent;

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
