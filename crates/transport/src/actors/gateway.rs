use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    request_response::OutboundRequestId,
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::{Heartbeat, Query, QueryResult};

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent},
        request_client::{ClientBehaviour, ClientConfig, ClientEvent, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    protocol::{MAX_QUERY_RESULT_SIZE, MAX_QUERY_SIZE, QUERY_PROTOCOL},
    record_event,
    util::{new_queue, Receiver, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
    QueueFull,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayEvent {
    Ping {
        peer_id: PeerId,
        ping: Heartbeat,
    },
    QueryResult {
        peer_id: PeerId,
        result: Result<QueryResult, QueryFailure>,
    },
    QueryDropped {
        query_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryFailure {
    Timeout(Timeout),
    ValidationError(String),
    TransportError(String),
}

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    query: Wrapped<ClientBehaviour<ProtoCodec<Query, QueryResult>>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub query_config: ClientConfig,
    pub max_query_result_size: u64,
    pub queries_queue_size: usize,
    pub logs_queue_size: usize,
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl GatewayConfig {
    pub fn new() -> Self {
        Self {
            query_config: Default::default(),
            max_query_result_size: MAX_QUERY_RESULT_SIZE,
            queries_queue_size: 100,
            logs_queue_size: 100,
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct GatewayBehaviour {
    inner: InnerBehaviour,
    query_ids: BTreeMap<OutboundRequestId, String>,
    dropped_queries: VecDeque<String>,
}

impl GatewayBehaviour {
    pub fn new(mut base: BaseBehaviour, config: GatewayConfig) -> Wrapped<Self> {
        base.subscribe_pings();
        let inner = InnerBehaviour {
            base: base.into(),
            query: ClientBehaviour::new(
                ProtoCodec::new(MAX_QUERY_SIZE, config.max_query_result_size),
                QUERY_PROTOCOL,
                config.query_config,
            )
            .into(),
        };
        Self {
            inner,
            query_ids: Default::default(),
            dropped_queries: Default::default(),
        }
        .into()
    }
    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<GatewayEvent> {
        match ev {
            BaseBehaviourEvent::Ping { peer_id, ping } => self.on_ping(peer_id, ping),
            _ => None,
        }
    }

    fn on_ping(&mut self, peer_id: PeerId, ping: Heartbeat) -> Option<GatewayEvent> {
        log::debug!("Got ping from {peer_id}");
        log::trace!("{ping:?}");
        Some(GatewayEvent::Ping { peer_id, ping })
    }

    fn take_query_id(&mut self, req_id: &OutboundRequestId) -> Option<String> {
        let query_id = self.query_ids.remove(req_id);
        if query_id.is_none() {
            log::error!("Unknown request ID: {req_id}");
        }
        query_id
    }

    fn on_query_result(
        &mut self,
        peer_id: PeerId,
        result: QueryResult,
        req_id: OutboundRequestId,
    ) -> Option<GatewayEvent> {
        log::debug!("Got query result from {peer_id}: {result:?}");
        let query_id = self.take_query_id(&req_id)?;
        let result = match validate_query_result(&query_id, &result) {
            Ok(()) => Ok(result),
            Err(err) => {
                log::warn!("Invalid result for query {query_id} from peer {peer_id}: {err}");
                Err(QueryFailure::ValidationError(err.to_owned()))
            }
        };
        Some(GatewayEvent::QueryResult { peer_id, result })
    }

    fn on_query_failure(
        &mut self,
        peer_id: PeerId,
        req_id: OutboundRequestId,
        error: String,
    ) -> Option<GatewayEvent> {
        log::debug!("Query failure: {error} (peer_id={peer_id})");
        self.take_query_id(&req_id)?;
        Some(GatewayEvent::QueryResult {
            peer_id,
            result: Err(QueryFailure::TransportError(error)),
        })
    }

    fn on_query_event(&mut self, ev: ClientEvent<QueryResult>) -> Option<GatewayEvent> {
        match ev {
            ClientEvent::Response {
                peer_id,
                req_id,
                response,
            } => self.on_query_result(peer_id, response, req_id),
            ClientEvent::Timeout {
                req_id,
                peer_id,
                timeout,
            } => self.on_query_timeout(req_id, peer_id, timeout),
            ClientEvent::PeerUnknown { peer_id } => {
                self.inner.base.find_and_dial(peer_id);
                None
            }
            ClientEvent::Failure {
                peer_id,
                req_id,
                error,
            } => self.on_query_failure(peer_id, req_id, error),
        }
    }

    fn on_query_timeout(
        &mut self,
        req_id: OutboundRequestId,
        peer_id: PeerId,
        timeout: Timeout,
    ) -> Option<GatewayEvent> {
        let query_id = self.take_query_id(&req_id)?;
        log::debug!("Query {query_id} timed out");
        Some(GatewayEvent::QueryResult {
            peer_id,
            result: Err(QueryFailure::Timeout(timeout)),
        })
    }

    pub fn send_query(&mut self, peer_id: PeerId, mut query: Query) {
        log::debug!("Sending query {query:?} to {peer_id}");
        // Sign the query
        let query_id = query.query_id.clone();
        query
            .sign(self.inner.base.keypair(), peer_id)
            .expect("query should be valid to sign");

        // Validate query size
        let query_size = query.encoded_len() as u64;
        if query_size > MAX_QUERY_SIZE {
            return log::error!("Query size too large: {query_size}");
        }

        // Try to send the query
        if let Ok(req_id) = self.inner.query.try_send_request(peer_id, query) {
            self.query_ids.insert(req_id, query_id);
        } else {
            self.dropped_queries.push_back(query_id)
            // TODO: notify the waker
        }
    }
}

fn validate_query_result(query_id: &str, result: &QueryResult) -> Result<(), &'static str> {
    if result.encoded_len() == 0 {
        // Empty response is a sign of worker error
        Err("Empty response")
    } else if query_id != result.query_id {
        // Stored query ID must match the ID in result
        Err("Query ID mismatch")
    } else {
        Ok(())
    }
}

impl BehaviourWrapper for GatewayBehaviour {
    type Inner = InnerBehaviour;
    type Event = GatewayEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            InnerBehaviourEvent::Base(ev) => self.on_base_event(ev),
            InnerBehaviourEvent::Query(query_res) => self.on_query_event(query_res),
        };
        ev.map(ToSwarm::GenerateEvent)
    }

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        match self.dropped_queries.pop_front() {
            None => Poll::Pending,
            Some(query_id) => {
                Poll::Ready(Some(ToSwarm::GenerateEvent(GatewayEvent::QueryDropped { query_id })))
            }
        }
    }
}

struct GatewayTransport {
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    queries_rx: Receiver<(PeerId, Query)>,
    events_tx: Sender<GatewayEvent>,
}

impl GatewayTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting gateway P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
                Some((peer_id, query)) = self.queries_rx.recv() => self.swarm.behaviour_mut().send_query(peer_id, query),
            }
        }
        log::info!("Shutting down gateway P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<GatewayEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct GatewayTransportHandle {
    queries_tx: Sender<(PeerId, Query)>,
    _task_manager: Arc<TaskManager>,
}

impl GatewayTransportHandle {
    fn new(
        queries_tx: Sender<(PeerId, Query)>,
        transport: GatewayTransport,
        shutdown_timeout: Duration,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            queries_tx,
            _task_manager: Arc::new(task_manager),
        }
    }
    pub fn send_query(&self, peer_id: PeerId, query: Query) -> Result<(), QueueFull> {
        log::debug!("Queueing query {query:?}");
        self.queries_tx.try_send((peer_id, query))
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<GatewayBehaviour>>,
    config: GatewayConfig,
) -> (impl Stream<Item = GatewayEvent>, GatewayTransportHandle) {
    let (queries_tx, queries_rx) = new_queue(config.queries_queue_size, "queries");
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = GatewayTransport {
        swarm,
        queries_rx,
        events_tx,
    };
    let handle = GatewayTransportHandle::new(queries_tx, transport, config.shutdown_timeout);
    (events_rx, handle)
}
