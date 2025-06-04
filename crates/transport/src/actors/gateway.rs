use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use prost::Message;
use serde::{Deserialize, Serialize};

use sqd_messages::{Heartbeat, Query, QueryFinished, QueryResult};
use tokio_util::sync::CancellationToken;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, QUERY_PROTOCOL},
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT}, QueueFull,
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
        }
    }
}

pub struct GatewayTransport {
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    logs_rx: Receiver<QueryFinished>,
    events_tx: Sender<GatewayEvent>,
}

impl GatewayTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting worker P2P transport");
        loop {
            tokio::select! {
                 _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some(log) = self.logs_rx.recv() => self.publish_portal_logs(log),
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

    fn publish_portal_logs(&mut self, log: QueryFinished) {
        log::trace!("Sending log: {log:?}");
        self.swarm.behaviour_mut().base.publish_portal_logs(log);
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
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
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
    let (logs_tx, logs_rx) = new_queue(100, "portal_logs");

    let query_handle = swarm.behaviour().base.request_handle(QUERY_PROTOCOL, config.query_config);
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = GatewayTransport {
        swarm,
        logs_rx,
        events_tx,
    };
    let handle = GatewayTransportHandle::new(
        logs_tx,
        query_handle,
        transport,
        config.shutdown_timeout,
    );
    (events_rx, handle)
}

pub struct GatewayBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl GatewayBehaviour {
    pub fn new(mut base: BaseBehaviour, config: GatewayConfig) -> Wrapped<Self> {
        if config.worker_status_via_gossipsub {
            base.subscribe_heartbeats();
        }
        if config.worker_status_via_polling {
            base.start_pulling_heartbeats();
        }
        base.subscribe_portal_logs();

        Self { base: base.into() }.into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<GatewayEvent> {
        match ev {
            BaseBehaviourEvent::Heartbeat { peer_id, heartbeat } => {
                self.on_heartbeat(peer_id, heartbeat)
            }
            _ => None,
        }
    }

    fn on_heartbeat(&mut self, peer_id: PeerId, heartbeat: Heartbeat) -> Option<GatewayEvent> {
        log::debug!("Got heartbeat from {peer_id}");
        log::trace!("{heartbeat:?}");
        Some(GatewayEvent::Heartbeat { peer_id, heartbeat })
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
