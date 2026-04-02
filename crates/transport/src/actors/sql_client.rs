use std::time::Duration;

use futures::StreamExt;
use libp2p::{PeerId, Swarm, swarm::{NetworkBehaviour, SwarmEvent, ToSwarm}};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};
use crate::{BehaviourWrapper, behaviour::base::BaseBehaviourEvent};
use crate::behaviour::wrapped::TToSwarm;
use tokio::sync::broadcast::{self, Receiver, Sender};

use sqd_messages::{Query, QueryResult};

use crate::{
    behaviour::{
        base::BaseBehaviour,
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::Wrapped,
    },
    protocol::{MAX_SQL_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, SQL_QUERY_PROTOCOL},
    record_event,
    util::{TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogsCollectorEvent {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SQLQueryFailure {
    InvalidRequest(String),
    Timeout(Timeout),
    TransportError(String),
    InvalidResponse(String),
}

#[derive(Debug, Clone, Copy)]
pub struct SQLClientConfig {
    pub query_config: ClientConfig,
    pub shutdown_timeout: Duration,
}

impl Default for SQLClientConfig {
    fn default() -> Self {
        Self {
            query_config: ClientConfig {
                max_response_size: MAX_QUERY_RESULT_SIZE,
                ..Default::default()
            },
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct SQLClientTransport {
    _task_manager: TaskManager,
    query_handle: StreamClientHandle,
    tx: Sender<f32>,
}

impl SQLClientTransport {
    pub async fn send_sql_query(
        &self,
        peer_id: PeerId,
        query: Query,
    ) -> Result<QueryResult, SQLQueryFailure> {
        let query_len = query.encoded_len() as u64;
        if query_len > MAX_SQL_QUERY_MSG_SIZE {
            return Err(SQLQueryFailure::InvalidRequest(format!(
                "SQLQuery message too large ({query_len} bytes)"
            )));
        }

        let buf = query.encode_to_vec();
        let resp_buf = self.query_handle.request_response(peer_id, &buf).await?;

        if resp_buf.is_empty() {
            // Empty response is a sign of worker error
            log::warn!("Empty response for sql query from peer {peer_id}");
            return Err(SQLQueryFailure::InvalidResponse("Empty response".to_string()));
        }
        let result = QueryResult::decode(resp_buf.as_slice())
            .map_err(|e| SQLQueryFailure::InvalidResponse(e.to_string()))?;

        log::debug!("Got sql query result from {peer_id}");
        Ok(result)
    }

    pub fn get_connected_to_network(&self) -> Receiver<f32> {
        self.tx.subscribe()
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<SQLClientBehaviour>>,
    config: SQLClientConfig,
) -> SQLClientTransport {
    let query_handle = swarm
        .behaviour()
        .inner
        .base
        .request_handle(SQL_QUERY_PROTOCOL, config.query_config);

    let (tx, _) = broadcast::channel(16);
    let local_tx = tx.clone();
    let mut task_manager = TaskManager::new(config.shutdown_timeout);
    task_manager.spawn(|cancel_token| async move {
        log::info!("Starting SQL Client P2P transport");
        let stream = swarm.take_until(cancel_token.cancelled_owned());
        tokio::pin!(stream);
        while let Some(ev) = stream.next().await {
            log::trace!("Swarm event: {ev:?}");
            record_event(&ev);
            match ev {
                SwarmEvent::Behaviour(SQLClientEvent::Connect { confidence })=> {
                    log::debug!("Connect confidence: {confidence}");
                    let _ = local_tx.send(confidence);
                },
                _ => {}
            }
        }
        log::info!("Shutting down SQL Client P2P transport");
    });

    SQLClientTransport {
        _task_manager: task_manager,
        query_handle,
        tx
    }
}


#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
}

pub struct SQLClientBehaviour {
    inner: InnerBehaviour,

}

impl SQLClientBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.keep_all_connections_alive();
        Self { 
            inner: InnerBehaviour {
                base: base.into(),
            }
        }.into()
    }
}

#[derive(Debug)]
pub enum SQLClientEvent {
    Connect {
        confidence: f32
    }
}

impl BehaviourWrapper for SQLClientBehaviour {
    type Inner = InnerBehaviour;
    type Event = SQLClientEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            InnerBehaviourEvent::Base(base) => {
                match base {
                    BaseBehaviourEvent::NetworkConnected { confidence } => vec![SQLClientEvent::Connect { confidence }],
                    _ => vec![]
                }
            }
        };
        ev.into_iter().map(ToSwarm::GenerateEvent)
    }
}

impl From<RequestError> for SQLQueryFailure {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Timeout(t) => Self::Timeout(t),
            e => Self::TransportError(e.to_string()),
        }
    }
}
