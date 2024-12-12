use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{FutureExt, StreamExt};
use libp2p::{
    swarm::{NetworkBehaviour, SwarmEvent, ToSwarm},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use parking_lot::Mutex;
use prost::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;

use sqd_messages::{LogsRequest, QueryLogs};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use crate::{
    behaviour::{
        base::BaseBehaviour,
        request_client::{ClientBehaviour, ClientEvent, Timeout},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    codec::ProtoCodec,
    protocol::{MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, WORKER_LOGS_PROTOCOL},
    record_event, ClientConfig, QueueFull,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogsCollectorEvent {}

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
pub enum FetchLogsError {
    #[error("Timeout: {0}")]
    Timeout(Timeout),
    #[error("Failure: {0}")]
    Failure(String),
}

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    logs: Wrapped<ClientBehaviour<ProtoCodec<LogsRequest, QueryLogs>>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LogsCollectorConfig {
    pub request_config: ClientConfig,
}

impl Default for LogsCollectorConfig {
    fn default() -> Self {
        Self {
            request_config: Default::default(),
        }
    }
}

pub struct LogsCollectorBehaviour {
    inner: InnerBehaviour,
    resp_senders: HashMap<PeerId, oneshot::Sender<Result<QueryLogs, FetchLogsError>>>,
}

impl LogsCollectorBehaviour {
    pub fn new(
        base: BaseBehaviour,
        _local_peer_id: PeerId,
        config: LogsCollectorConfig,
    ) -> Wrapped<Self> {
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                logs: ClientBehaviour::new(
                    ProtoCodec::new(MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE),
                    WORKER_LOGS_PROTOCOL,
                    config.request_config,
                )
                .into(),
            },
            resp_senders: Default::default(),
        }
        .into()
    }

    fn on_worker_logs(
        &mut self,
        peer_id: PeerId,
        mut logs: QueryLogs,
    ) -> Option<LogsCollectorEvent> {
        log::debug!("Got {} query logs from {peer_id}", logs.queries_executed.len());
        logs.queries_executed.retain(|log| {
            if log.verify_client_signature(peer_id) {
                true
            } else {
                log::warn!("Invalid client signature in query log: {log:?}");
                false
            }
        });
        if let Some(sender) = self.resp_senders.remove(&peer_id) {
            if sender.send(Ok(logs)).is_err() {
                log::warn!("Logs response channel closed for {peer_id}");
            }
        } else {
            log::warn!("Not expecting query logs for peer {peer_id}");
        }
        None
    }

    fn on_failure(&mut self, peer_id: PeerId, error: FetchLogsError) -> Option<LogsCollectorEvent> {
        log::debug!("Couldn't get query logs from {peer_id}: {error:?}");
        if let Some(sender) = self.resp_senders.remove(&peer_id) {
            sender.send(Err(error)).ok();
        } else {
            log::warn!("Not expecting query logs for peer {peer_id}");
        }
        None
    }

    pub fn request_logs(
        &mut self,
        peer_id: PeerId,
        request: LogsRequest,
        resp_tx: oneshot::Sender<Result<QueryLogs, FetchLogsError>>,
    ) -> Result<(), QueueFull> {
        let request_size = request.encoded_len() as u64;
        if request_size > MAX_LOGS_REQUEST_SIZE {
            log::error!("Logs request size too large: {request_size}");
            return Ok(());
        }

        log::debug!(
            "Requesting logs from {peer_id} from {}, last query id: {:?}",
            request.from_timestamp_ms,
            request.last_received_query_id
        );

        let prev = self.resp_senders.insert(peer_id, resp_tx);
        if prev.is_some() {
            log::warn!("Dropping ongoing logs request to {peer_id}");
        }

        self.inner.logs.try_send_request(peer_id, request)?;
        Ok(())
    }
}

impl BehaviourWrapper for LogsCollectorBehaviour {
    type Inner = InnerBehaviour;
    type Event = LogsCollectorEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            InnerBehaviourEvent::Base(_) => None,
            InnerBehaviourEvent::Logs(client_event) => match client_event {
                ClientEvent::Response {
                    peer_id, response, ..
                } => self.on_worker_logs(peer_id, response),
                ClientEvent::Timeout {
                    peer_id, timeout, ..
                } => self.on_failure(peer_id, FetchLogsError::Timeout(timeout)),
                ClientEvent::PeerUnknown { peer_id } => {
                    self.inner.base.find_and_dial(peer_id);
                    None
                }
                ClientEvent::Failure { peer_id, error, .. } => {
                    self.on_failure(peer_id, FetchLogsError::Failure(error))
                }
            },
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

pub struct LogsCollectorTransport {
    swarm: Mutex<Swarm<Wrapped<LogsCollectorBehaviour>>>,
}

impl LogsCollectorTransport {
    pub async fn request_logs(
        &self,
        peer_id: PeerId,
        request: LogsRequest,
    ) -> Result<QueryLogs, FetchLogsError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.swarm
            .lock()
            .behaviour_mut()
            .request_logs(peer_id, request, resp_tx)
            .map_err(|_| FetchLogsError::Failure("Logs request queue full".to_string()))?;
        resp_rx
            .await
            .map_err(|_| FetchLogsError::Failure("Logs response channel closed".to_string()))?
    }

    pub fn run(&self, cancel_token: CancellationToken) -> LogsCollectorRunFuture {
        log::info!("Starting logs collector P2P transport");
        LogsCollectorRunFuture {
            transport: self,
            cancelled: Box::pin(cancel_token.cancelled_owned()),
        }
    }

    fn on_swarm_event(&self, ev: SwarmEvent<LogsCollectorEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<LogsCollectorBehaviour>>,
    _config: LogsCollectorConfig,
) -> LogsCollectorTransport {
    let transport = LogsCollectorTransport {
        swarm: Mutex::new(swarm),
    };
    transport
}

pub struct LogsCollectorRunFuture<'t> {
    transport: &'t LogsCollectorTransport,
    cancelled: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl<'t> Future for LogsCollectorRunFuture<'t> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.cancelled.poll_unpin(cx).is_ready() {
            log::info!("Shutting down logs collector P2P transport");
            return Poll::Ready(());
        }
        loop {
            match self.transport.swarm.lock().poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(ev)) => self.transport.on_swarm_event(ev),
                Poll::Ready(None) => unreachable!("Swarm stream ended"),
            }
        }
    }
}
