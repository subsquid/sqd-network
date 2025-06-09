use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use futures_core::Stream;
use libp2p::{
    request_response::ResponseChannel, swarm::{NetworkBehaviour, SwarmEvent, ToSwarm}, PeerId, Swarm
};
use libp2p_swarm_derive::NetworkBehaviour;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use sqd_messages::QueryFinished;

use crate::{
    behaviour::{
        base::{BaseBehaviour, BaseBehaviourEvent}, request_server::{Request, ServerBehaviour}, wrapped::{BehaviourWrapper, TToSwarm, Wrapped}
    }, codec::ProtoCodec, protocol::{MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE, PORTAL_LOGS_TOPIC}, record_event, util::{new_queue, Sender, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT}
};

#[derive(Debug)]
pub enum PortalLogsCollectorEvent {
    Log { peer_id: PeerId, log: QueryFinished },
    LogQuery { peer_id: PeerId, query: QueryFinished},
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PortalLogsCollectorConfig {
    pub events_queue_size: usize,
    pub shutdown_timeout: Duration,
}

impl PortalLogsCollectorConfig {
    pub fn new() -> Self {
        Self {
            events_queue_size: 100,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

type PortalLogCollectionBehaviour = Wrapped<ServerBehaviour<ProtoCodec<QueryFinished, ()>>>;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    base: Wrapped<BaseBehaviour>,
    collector: PortalLogCollectionBehaviour,
}

pub struct PortalLogsCollectorBehaviour {
    inner: InnerBehaviour,
}

impl PortalLogsCollectorBehaviour {
    pub fn new(mut base: BaseBehaviour) -> Wrapped<Self> {
        base.set_server_mode();
        base.subscribe_portal_logs();
        base.claim_portal_logs_listener_role();
        Self {
            inner: InnerBehaviour {
                base: base.into(),
                collector: ServerBehaviour::new(
                    ProtoCodec::new(MAX_QUERY_MSG_SIZE, MAX_QUERY_RESULT_SIZE),
                    PORTAL_LOGS_TOPIC,
                    Duration::from_secs(5),
                )
                .into(),
            }
        }.into()
    }

    fn on_base_event(&mut self, ev: BaseBehaviourEvent) -> Option<PortalLogsCollectorEvent> {
        match ev {
            BaseBehaviourEvent::PortalLogs { peer_id, log }  => Some(PortalLogsCollectorEvent::Log { peer_id, log }),
            _ => None,
        }
    }

    fn on_collector_request(
        &mut self,
        peer_id: PeerId,
        query: QueryFinished,
        resp_chan: ResponseChannel<()>,
    ) -> Option<PortalLogsCollectorEvent> {
        let _ = self.inner.collector.try_send_response(resp_chan, ());
        Some(PortalLogsCollectorEvent::LogQuery {
            peer_id,
            query,
        })
    }
}

impl Drop for PortalLogsCollectorBehaviour {
    fn drop(&mut self) {
        self.inner().base.relinquish_portal_logs_listener_role();
    }
}

impl BehaviourWrapper for PortalLogsCollectorBehaviour {
    type Inner = InnerBehaviour;
    type Event = PortalLogsCollectorEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        let ev = match ev {
            InnerBehaviourEvent::Base(ev) => self.on_base_event(ev),
            InnerBehaviourEvent::Collector(Request {
                peer_id,
                request,
                response_channel,
            }) => self.on_collector_request(peer_id, request, response_channel),
        };
        ev.map(ToSwarm::GenerateEvent)
    }
}

struct PortalLogsCollectorTransport {
    swarm: Swarm<Wrapped<PortalLogsCollectorBehaviour>>,
    events_tx: Sender<PortalLogsCollectorEvent>,
}

impl PortalLogsCollectorTransport {
    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("Starting observer P2P transport");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                ev = self.swarm.select_next_some() => self.on_swarm_event(ev),
            }
        }
        log::info!("Shutting down observer P2P transport");
    }

    fn on_swarm_event(&mut self, ev: SwarmEvent<PortalLogsCollectorEvent>) {
        log::trace!("Swarm event: {ev:?}");
        record_event(&ev);
        if let SwarmEvent::Behaviour(ev) = ev {
            self.events_tx.send_lossy(ev)
        }
    }
}

#[derive(Clone)]
pub struct PortalLogsCollectorTransportHandle {
    _task_manager: Arc<TaskManager>,
}

impl PortalLogsCollectorTransportHandle {
    fn new(transport: PortalLogsCollectorTransport, shutdown_timeout: Duration) -> Self {
        let mut task_manager = TaskManager::new(shutdown_timeout);
        task_manager.spawn(|c| transport.run(c));
        Self {
            _task_manager: Arc::new(task_manager),
        }
    }
}

pub fn start_transport(
    swarm: Swarm<Wrapped<PortalLogsCollectorBehaviour>>,
    config: PortalLogsCollectorConfig,
) -> (impl Stream<Item = PortalLogsCollectorEvent>, PortalLogsCollectorTransportHandle) {
    let (events_tx, events_rx) = new_queue(config.events_queue_size, "events");
    let transport = PortalLogsCollectorTransport { swarm, events_tx };
    let handle = PortalLogsCollectorTransportHandle::new(transport, config.shutdown_timeout);
    (events_rx, handle)
}
