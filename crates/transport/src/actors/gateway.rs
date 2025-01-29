use std::time::Duration;

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use prost::Message;
use serde::{Deserialize, Serialize};

use sqd_messages::{Heartbeat, Query, QueryResult};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        stream_client::{RequestError, ClientConfig, StreamClientHandle, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    protocol::{MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, QUERY_PROTOCOL},
    record_event,
    util::{new_queue, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
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
        }
    }
}

pub struct GatewayTransport {
    _task_manager: TaskManager,
    query_handle: StreamClientHandle,
}

impl GatewayTransport {
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
        let resp_buf = self
            .query_handle
            .request_response(peer_id, &buf)
            .await?;

        if resp_buf.len() == 0 {
            // Empty response is a sign of worker error
            log::warn!("Empty response for query from peer {peer_id}");
            return Err(QueryFailure::InvalidResponse("Empty response".to_string()));
        }
        let result = QueryResult::decode(resp_buf.as_slice())
            .map_err(|e| QueryFailure::InvalidResponse(e.to_string()))?;

        log::debug!("Got query result from {peer_id}");
        Ok(result)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    config: GatewayConfig,
) -> (impl Stream<Item = GatewayEvent>, GatewayTransport) {
    let query_handle = swarm.behaviour().base.request_handle(QUERY_PROTOCOL, config.query_config);
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");

    let mut task_manager = TaskManager::new(config.shutdown_timeout);
    task_manager.spawn(|cancel_token| async move {
        log::info!("Starting gateway P2P transport");
        let stream = swarm.take_until(cancel_token.cancelled_owned());
        tokio::pin!(stream);
        while let Some(ev) = stream.next().await {
            log::trace!("Swarm event: {ev:?}");
            record_event(&ev);
            if let SwarmEvent::Behaviour(ev) = ev {
                events_tx.send_lossy(ev)
            }
        }
        log::info!("Shutting down gateway P2P transport");
    });

    let transport = GatewayTransport {
        _task_manager: task_manager,
        query_handle,
    };
    (events_rx, transport)
}

pub struct GatewayBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl GatewayBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.subscribe_heartbeats();

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
            RequestError::Timeout(t) => QueryFailure::Timeout(t),
            e => QueryFailure::TransportError(e.to_string()),
        }
    }
}
