use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Swarm};
use prost::Message;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_contract_client::PeerId;

use crate::{
    behaviour::{
        base::BaseBehaviour,
        stream_client::{ClientConfig, RequestError, StreamClientHandle},
        wrapped::{BehaviourWrapper, Wrapped},
    },
    protocol::{MAX_HEARTBEAT_SIZE, WORKER_STATUS_PROTOCOL},
    record_event,
    util::{TaskManager, DEFAULT_SHUTDOWN_TIMEOUT},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub peer_id: PeerId,
    pub heartbeat: sqd_messages::Heartbeat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PingsCollectorConfig {
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
    /// Timeout for individual heartbeat requests (default: 15 sec)
    pub request_timeout: Duration,
    /// Timeout for connecting to peers (default: 10 sec)
    pub connect_timeout: Duration,
}

impl Default for PingsCollectorConfig {
    fn default() -> Self {
        Self {
            events_queue_size: 1000,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            request_timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

pub struct PingsCollectorBehaviour {
    base: Wrapped<BaseBehaviour>,
}

impl PingsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour, _config: PingsCollectorConfig) -> Wrapped<Self> {
        base.keep_all_connections_alive();
        Self { base: base.into() }.into()
    }
}

impl BehaviourWrapper for PingsCollectorBehaviour {
    type Inner = Wrapped<BaseBehaviour>;
    type Event = ();

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.base
    }
}

struct PingsCollectorTransport {
    swarm: Swarm<Wrapped<PingsCollectorBehaviour>>,
}

impl PingsCollectorTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting pings collector P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down pings collector P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<()>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
    }
}

#[derive(Clone)]
pub struct PingsCollectorTransportHandle {
    _task_manager: Arc<TaskManager>,
    stream_handle: StreamClientHandle,
}

impl PingsCollectorTransportHandle {
    fn new(
        transport: PingsCollectorTransport,
        shutdown_timeout: Duration,
        stream_handle: StreamClientHandle,
    ) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
            stream_handle,
        }
    }

    /// Request a heartbeat from a specific peer
    pub async fn request_heartbeat(
        &self,
        peer_id: PeerId,
    ) -> Result<sqd_messages::Heartbeat, RequestError> {
        log::debug!("Requesting heartbeat from {peer_id}");
        let resp = self.stream_handle.request_response(peer_id, b"").await?;
        let heartbeat = sqd_messages::Heartbeat::decode(resp.as_ref()).map_err(|e| {
            RequestError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Error decoding heartbeat: {e}"),
            ))
        })?;
        log::debug!("Got heartbeat from {peer_id}: {heartbeat:?}");
        Ok(heartbeat)
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PingsCollectorBehaviour>>,
    config: PingsCollectorConfig,
) -> PingsCollectorTransportHandle {
    let stream_handle = swarm.behaviour().base.request_handle(
        WORKER_STATUS_PROTOCOL,
        ClientConfig {
            max_concurrent_streams: None,
            max_response_size: MAX_HEARTBEAT_SIZE,
            request_timeout: config.request_timeout,
            connect_timeout: config.connect_timeout,
            ..Default::default()
        },
    );
    let transport = PingsCollectorTransport { swarm };
    PingsCollectorTransportHandle::new(transport, config.shutdown_timeout, stream_handle)
}
