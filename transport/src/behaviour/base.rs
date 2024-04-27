use std::{
    collections::HashMap,
    task::{Context, Poll},
    time::Duration,
    vec,
};

use bimap::BiHashMap;
use futures_bounded::FuturesMap;
use libp2p::{
    allow_block_list,
    allow_block_list::BlockedPeers,
    autonat,
    autonat::NatStatus,
    core::ConnectedPoint,
    dcutr, identify,
    identity::Keypair,
    kad,
    kad::{
        store::MemoryStore, GetClosestPeersError, GetClosestPeersOk, ProgressStep, QueryId,
        QueryResult,
    },
    ping, relay, request_response,
    request_response::ProtocolSupport,
    swarm::{
        behaviour::ConnectionEstablished, ConnectionClosed, FromSwarm, NetworkBehaviour, ToSwarm,
    },
    StreamProtocol,
};
use libp2p_swarm_derive::NetworkBehaviour;
use prost::Message;
use serde::{Deserialize, Serialize};

use subsquid_messages::{broadcast_msg, BroadcastMsg, Envelope, LogsCollected, Ping};

use crate::{
    behaviour::{
        pubsub::{PubsubBehaviour, PubsubMsg},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    cli::BootNode,
    codec::LegacyCodec,
    protocol::{DHT_PROTOCOL, ID_PROTOCOL, LEGACY_PROTOCOL, LOGS_TOPIC, PING_TOPIC},
    util::addr_is_reachable,
    PeerId, QueueFull,
};

#[cfg(feature = "metrics")]
use crate::metrics::ONGOING_QUERIES;
#[cfg(feature = "metrics")]
use libp2p::metrics::{Metrics, Recorder};
#[cfg(feature = "metrics")]
use prometheus_client::registry::Registry;

pub const ACK_SIZE: u64 = 4;

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    relay: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    block: allow_block_list::Behaviour<BlockedPeers>,
    pubsub: Wrapped<PubsubBehaviour>,
    // Legacy behaviour for backward compatibility
    legacy: request_response::Behaviour<LegacyCodec<Vec<u8>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseConfig {
    pub autonat_timeout: Duration,
    pub identify_interval: Duration,
    pub request_timeout: Duration,
    pub probe_timeout: Duration,
    pub max_concurrent_probes: usize,
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            autonat_timeout: Duration::from_secs(60),
            identify_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(60),
            probe_timeout: Duration::from_secs(60),
            max_concurrent_probes: 1000,
        }
    }
}

pub struct BaseBehaviour {
    inner: InnerBehaviour,
    ongoing_queries: BiHashMap<PeerId, QueryId>,
    outbound_conns: HashMap<PeerId, u32>,
    probe_timeouts: FuturesMap<PeerId, ()>,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
}

impl BaseBehaviour {
    pub fn new(
        keypair: &Keypair,
        config: BaseConfig,
        boot_nodes: Vec<BootNode>,
        relay: relay::client::Behaviour,
        #[cfg(feature = "metrics")] registry: &mut Registry,
    ) -> Self {
        let local_peer_id = keypair.public().to_peer_id();
        let mut inner = InnerBehaviour {
            identify: identify::Behaviour::new(
                identify::Config::new(ID_PROTOCOL.to_string(), keypair.public())
                    .with_interval(config.identify_interval)
                    .with_push_listen_addr_updates(true),
            ),
            kademlia: kad::Behaviour::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                kad::Config::new(DHT_PROTOCOL),
            ),
            relay,
            dcutr: dcutr::Behaviour::new(local_peer_id),
            ping: ping::Behaviour::new(ping::Config::default()),
            autonat: autonat::Behaviour::new(
                local_peer_id,
                autonat::Config {
                    timeout: config.autonat_timeout,
                    ..Default::default()
                },
            ),
            block: Default::default(),
            pubsub: PubsubBehaviour::new(
                keypair.clone(),
                #[cfg(feature = "metrics")]
                registry,
            )
            .into(),
            legacy: request_response::Behaviour::new(
                vec![(LEGACY_PROTOCOL, ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(config.request_timeout),
            ),
        };

        for boot_node in boot_nodes {
            inner.kademlia.add_address(&boot_node.peer_id, boot_node.address.clone());
            inner.autonat.add_server(boot_node.peer_id, Some(boot_node.address));
        }

        Self {
            inner,
            ongoing_queries: Default::default(),
            outbound_conns: Default::default(),
            probe_timeouts: FuturesMap::new(config.probe_timeout, config.max_concurrent_probes),
            #[cfg(feature = "metrics")]
            metrics: Metrics::new(registry),
        }
    }
    pub fn subscribe_pings(&mut self) {
        self.inner.pubsub.subscribe(PING_TOPIC, false);
    }

    pub fn subscribe_logs_collected(&mut self) {
        self.inner.pubsub.subscribe(LOGS_TOPIC, false);
    }

    pub fn publish_ping(&mut self, ping: Ping) {
        self.inner.pubsub.publish(PING_TOPIC, ping.encode_to_vec());
    }

    pub fn publish_logs_collected(&mut self, logs_collected: LogsCollected) {
        self.inner.pubsub.publish(LOGS_TOPIC, logs_collected.encode_to_vec());
    }

    pub fn send_legacy_msg(&mut self, peer_id: &PeerId, msg: impl Message) {
        log::debug!("Sending msg to {peer_id}");
        let msg = msg.encode_to_vec();
        self.inner.legacy.send_request(peer_id, msg);
    }

    pub fn find_and_dial(&mut self, peer_id: PeerId) {
        if self.ongoing_queries.contains_left(&peer_id) {
            log::debug!("Query for peer {peer_id} already ongoing");
        } else {
            log::debug!("Starting query for peer {peer_id}");
            let query_id = self.inner.kademlia.get_closest_peers(peer_id);
            self.ongoing_queries.insert(peer_id, query_id);
            #[cfg(feature = "metrics")]
            ONGOING_QUERIES.inc();
        }
    }

    /// Try to probe if peer is reachable. Returns:
    ///   * Ok(true) if there is an established outbound connection to peer,
    ///   * Ok(false) if a probe has been scheduled,
    ///   * Err(QueueFull) if probe cannot be scheduled.
    pub fn try_probe_peer(&mut self, peer_id: PeerId) -> Result<bool, QueueFull> {
        if self.outbound_conns.get(&peer_id).is_some_and(|x| *x > 0) {
            log::debug!("Outbound connection to {peer_id} already exists");
            return Ok(true);
        }
        if self.probe_timeouts.contains(peer_id) {
            log::debug!("Probe for peer {peer_id} already ongoing");
            return Ok(false);
        }
        if self.probe_timeouts.try_push(peer_id, futures::future::pending()).is_err() {
            return Err(QueueFull);
        }
        log::debug!("Probing peer {peer_id}");
        self.find_and_dial(peer_id);
        Ok(false)
    }

    pub fn block_peer(&mut self, peer_id: PeerId) {
        log::info!("Blocking peer {peer_id}");
        self.inner.block.block_peer(peer_id);
    }
}

#[derive(Debug, Clone)]
pub enum BaseBehaviourEvent {
    BroadcastMsg {
        peer_id: PeerId,
        msg: BroadcastMsg,
    },
    LegacyMsg {
        peer_id: PeerId,
        envelope: Envelope,
    },
    PeerProbed {
        peer_id: PeerId,
        reachable: bool,
    },
    PeerProtocols {
        peer_id: PeerId,
        protocols: Vec<StreamProtocol>,
    },
}

impl BehaviourWrapper for BaseBehaviour {
    type Inner = InnerBehaviour;
    type Event = BaseBehaviourEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_swarm_event(&mut self, ev: FromSwarm) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                endpoint: ConnectedPoint::Dialer { .. },
                ..
            }) => self.on_outbound_established(peer_id),
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                endpoint: ConnectedPoint::Dialer { .. },
                ..
            }) => self.on_outbound_closed(peer_id),
            _ => None,
        }
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            InnerBehaviourEvent::Identify(ev) => self.on_identify_event(ev),
            InnerBehaviourEvent::Kademlia(ev) => self.on_kademlia_event(ev),
            InnerBehaviourEvent::Autonat(ev) => self.on_autonat_event(ev),
            InnerBehaviourEvent::Pubsub(ev) => self.on_pubsub_event(ev),
            InnerBehaviourEvent::Legacy(ev) => self.on_legacy_event(ev),
            #[cfg(feature = "metrics")]
            InnerBehaviourEvent::Ping(ev) => {
                self.metrics.record(&ev);
                None
            }
            #[cfg(feature = "metrics")]
            InnerBehaviourEvent::Dcutr(ev) => {
                self.metrics.record(&ev);
                None
            }
            _ => None,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        match self.probe_timeouts.poll_unpin(cx) {
            Poll::Ready((peer_id, Err(_))) => {
                log::debug!("Probe for peer {peer_id} timed out");
                Poll::Ready(Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::PeerProbed {
                    peer_id,
                    reachable: false,
                })))
            }
            Poll::Pending => Poll::Pending,
            _ => unreachable!(), // future::pending() should never complete
        }
    }
}

impl BaseBehaviour {
    fn on_outbound_established(&mut self, peer_id: PeerId) -> Option<TToSwarm<Self>> {
        log::debug!("Established outbound connection to {peer_id}");
        *self.outbound_conns.entry(peer_id).or_default() += 1;
        if self.probe_timeouts.remove(peer_id).is_some() {
            Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::PeerProbed {
                peer_id,
                reachable: true,
            }))
        } else {
            None
        }
    }

    fn on_outbound_closed(&mut self, peer_id: PeerId) -> Option<TToSwarm<Self>> {
        log::debug!("Closed outbound connection to {peer_id}");
        match self.outbound_conns.get_mut(&peer_id) {
            Some(x) => *x -= 1,
            None => log::error!("Closed connection not established before"),
        }
        None
    }

    fn on_identify_event(&mut self, ev: identify::Event) -> Option<TToSwarm<Self>> {
        log::debug!("Identify event received: {ev:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&ev);
        let (peer_id, listen_addrs, protocols) = match ev {
            identify::Event::Received { peer_id, info } => {
                (peer_id, info.listen_addrs, info.protocols)
            }
            _ => return None,
        };
        let kademlia = &mut self.inner.kademlia;
        listen_addrs.into_iter().filter(addr_is_reachable).for_each(|addr| {
            kademlia.add_address(&peer_id, addr);
        });
        let ev = BaseBehaviourEvent::PeerProtocols { peer_id, protocols };
        Some(ToSwarm::GenerateEvent(ev))
    }

    fn on_kademlia_event(&mut self, ev: kad::Event) -> Option<TToSwarm<Self>> {
        log::debug!("Kademlia event received: {ev:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&ev);
        let (query_id, result, finished) = match ev {
            kad::Event::OutboundQueryProgressed {
                id,
                result: QueryResult::GetClosestPeers(result),
                step: ProgressStep { last, .. },
                ..
            } => (id, result, last),
            _ => return None,
        };

        let peer_id = match self.ongoing_queries.get_by_right(&query_id) {
            None => return None,
            Some(peer_id) => peer_id.to_owned(),
        };
        let peers = match result {
            Ok(GetClosestPeersOk { peers, .. }) => peers,
            Err(GetClosestPeersError::Timeout { peers, .. }) => peers,
        };

        // Query reached the peer that was looked for. Try to dial the peer now.
        if peers.contains(&peer_id) {
            self.ongoing_queries.remove_by_right(&query_id);
            #[cfg(feature = "metrics")]
            ONGOING_QUERIES.dec();
            return Some(ToSwarm::Dial {
                opts: peer_id.into(),
            });
        }
        // Query finished and the peer wasn't found.
        else if finished {
            self.ongoing_queries.remove_by_right(&query_id);
            #[cfg(feature = "metrics")]
            ONGOING_QUERIES.dec();
        }
        None
    }

    fn on_autonat_event(&mut self, ev: autonat::Event) -> Option<TToSwarm<Self>> {
        log::debug!("AutoNAT event received: {ev:?}");
        let status = match ev {
            autonat::Event::StatusChanged { new, .. } => new,
            _ => return None,
        };
        match status {
            NatStatus::Public(addr) => log::info!("Public address confirmed: {addr}"),
            NatStatus::Private => log::warn!("Public address check failed."),
            NatStatus::Unknown => {}
        }
        None
    }

    fn on_pubsub_event(
        &mut self,
        PubsubMsg {
            peer_id,
            topic,
            data,
        }: PubsubMsg,
    ) -> Option<TToSwarm<Self>> {
        log::debug!("Pub-sub message received: peer_id={peer_id} topic={topic}");
        let msg = match topic {
            PING_TOPIC => {
                let ping = match Ping::decode(data.as_ref()) {
                    Ok(ping) => ping,
                    Err(e) => {
                        log::error!("Error decoding ping: {e:?}");
                        return None;
                    }
                };
                BroadcastMsg {
                    msg: Some(broadcast_msg::Msg::Ping(ping)),
                }
            }
            LOGS_TOPIC => {
                let logs_collected = match LogsCollected::decode(data.as_ref()) {
                    Ok(logs_collected) => logs_collected,
                    Err(e) => {
                        log::error!("Error decoding logs collected: {e:?}");
                        return None;
                    }
                };
                BroadcastMsg {
                    msg: Some(broadcast_msg::Msg::LogsCollected(logs_collected)),
                }
            }
            _ => return None,
        };
        let ev = BaseBehaviourEvent::BroadcastMsg { peer_id, msg };
        Some(ToSwarm::GenerateEvent(ev))
    }

    fn on_legacy_event(
        &mut self,
        ev: request_response::Event<Vec<u8>, u8>,
    ) -> Option<TToSwarm<Self>> {
        log::debug!("Request-Response event received: {ev:?}");
        let (peer_id, msg_content, channel) = match ev {
            request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
            } => (peer, request, channel),
            request_response::Event::InboundFailure { error, peer, .. } => {
                log::error!("Inbound message failure (peer_id={peer}): {error:?}");
                return None;
            }
            request_response::Event::OutboundFailure { error, peer, .. } => {
                log::error!("Outbound message failure: (peer_id={peer}): {error:?}");
                return None;
            }
            _ => return None,
        };

        // Send minimal response to prevent errors being emitted on the sender side
        _ = self.inner.legacy.send_response(channel, 1u8);

        // Parse the message and queue the event
        let envelope = match Envelope::decode(msg_content.as_slice()) {
            Ok(envelope) => envelope,
            Err(e) => {
                log::error!("Error decoding message from {peer_id}: {e:?}");
                return None;
            }
        };
        let ev = BaseBehaviourEvent::LegacyMsg { peer_id, envelope };
        Some(ToSwarm::GenerateEvent(ev))
    }
}
