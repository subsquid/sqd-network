use std::time::Duration;

use futures::StreamExt;
use libp2p::{PeerId, Swarm};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use sqd_messages::{LogsRequest, QueryLogs};

use crate::{
    behaviour::{
        base::BaseBehaviour,
        stream_client::{ClientConfig, RequestError, StreamClientHandle, Timeout},
        wrapped::Wrapped,
    },
    protocol::{MAX_LOGS_REQUEST_SIZE, MAX_LOGS_RESPONSE_SIZE, WORKER_LOGS_PROTOCOL},
    record_event,
    util::{TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogsCollectorEvent {}

#[derive(Debug, Clone, Error)]
pub enum FetchLogsError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Timeout: {0}")]
    Timeout(Timeout),
    #[error("Failure: {0}")]
    Failure(String),
    #[error("Couldn't parse response: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Clone, Copy)]
pub struct LogsCollectorConfig {
    pub request_config: ClientConfig,
    pub shutdown_timeout: Duration,
}

impl Default for LogsCollectorConfig {
    fn default() -> Self {
        Self {
            request_config: ClientConfig {
                max_response_size: MAX_LOGS_RESPONSE_SIZE,
                ..Default::default()
            },
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

pub struct LogsCollectorTransport {
    _task_manager: TaskManager,
    request_handle: StreamClientHandle,
}

impl LogsCollectorTransport {
    pub async fn request_logs(
        &self,
        peer_id: PeerId,
        request: LogsRequest,
    ) -> Result<QueryLogs, FetchLogsError> {
        let request_len = request.encoded_len() as u64;
        if request_len > MAX_LOGS_REQUEST_SIZE {
            return Err(FetchLogsError::InvalidRequest(format!(
                "LogsRequest message too large ({request_len} bytes)"
            )));
        }

        let buf = request.encode_to_vec();
        let resp_buf = self
            .request_handle
            .request_response(peer_id, &buf)
            .await
            .map_err(FetchLogsError::from)?;

        let resp = QueryLogs::decode(resp_buf.as_slice())
            .map_err(|e| FetchLogsError::InvalidResponse(e.to_string()))?;

        Ok(resp)
    }
}

pub fn start_transport(
    swarm: Swarm<LogsCollectorBehaviour>,
    config: LogsCollectorConfig,
) -> LogsCollectorTransport {
    let request_handle = swarm
        .behaviour()
        .base
        .request_handle(WORKER_LOGS_PROTOCOL, config.request_config);

    let mut task_manager = TaskManager::new(config.shutdown_timeout);
    task_manager.spawn(|cancel_token| async move {
        log::info!("Starting logs collector P2P transport");
        let stream = swarm.take_until(cancel_token.cancelled_owned());
        tokio::pin!(stream);
        while let Some(ev) = stream.next().await {
            log::trace!("Swarm event: {ev:?}");
            record_event(&ev);
        }
        log::info!("Shutting down logs collector P2P transport");
    });

    LogsCollectorTransport {
        _task_manager: task_manager,
        request_handle,
    }
}

#[derive(NetworkBehaviour)]
pub struct LogsCollectorBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl LogsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Self {
        base.keep_all_connections_alive();
        Self { base: base.into() }
    }
}

impl From<RequestError> for FetchLogsError {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::Timeout(t) => Self::Timeout(t),
            e => Self::Failure(e.to_string()),
        }
    }
}
