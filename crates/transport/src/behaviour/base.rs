use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
    vec,
};

use bimap::BiHashMap;
use futures::{Stream, StreamExt};
use futures_bounded::FuturesMap;
use libp2p::{
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
    ping, relay,
    swarm::{
        behaviour::ConnectionEstablished,
        dial_opts::{DialOpts, PeerCondition},
        ConnectionClosed, ConnectionId, DialFailure, FromSwarm, NetworkBehaviour, ToSwarm,
    },
    StreamProtocol,
};
use libp2p_swarm_derive::NetworkBehaviour;
use parking_lot::RwLock;
use prost::Message;
use serde::{Deserialize, Serialize};

use sqd_contract_client::{Client as ContractClient, NetworkNodes};
use sqd_messages::Heartbeat;

#[cfg(feature = "metrics")]
use crate::metrics::{
    ACTIVE_CONNECTIONS, HEARTBEATS_PUBLISHED, HEARTBEATS_RECEIVED, ONGOING_PROBES, ONGOING_QUERIES,
};
use crate::{
    behaviour::{
        addr_cache::AddressCache,
        node_whitelist::{WhitelistBehavior, WhitelistConfig},
        pubsub::{PubsubBehaviour, PubsubMsg},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    cli::BootNode,
    protocol::{
        HEARTBEATS_MIN_INTERVAL, HEARTBEAT_TOPIC, ID_PROTOCOL, MAX_HEARTBEAT_SIZE,
        MAX_PUBSUB_MSG_SIZE, WORKER_STATUS_PROTOCOL,
    },
    record_event,
    util::{addr_is_reachable, parse_env_var},
    AgentInfo, Multiaddr, PeerId,
};

use super::{
    pubsub::{MsgValidationConfig, ValidationError},
    stream_client::{self, ClientBehaviour, ClientConfig, StreamClientHandle},
};

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    relay: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    whitelist: Wrapped<WhitelistBehavior>,
    pubsub: Wrapped<PubsubBehaviour>,
    address_cache: AddressCache,
    stream: Wrapped<ClientBehaviour>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BaseConfig {
    /// How often to check for on-chain updates: current epoch and registered nodes (default: 3 min).
    pub onchain_update_interval: Duration,
    /// Timeout for autoNAT probes (default: 60 sec).
    pub autonat_timeout: Duration,
    /// How often to publish identify info to connected nodes (default: 60 sec).
    pub identify_interval: Duration,
    /// Timeout for outgoing reachability probes (default: 20 sec).
    pub probe_timeout: Duration,
    /// Timeout for kademlia DHT queries (default: 10 sec).
    pub kad_query_timeout: Duration,
    /// Maximum number of concurrent outgoing reachability probes (default: 1024)
    pub max_concurrent_probes: usize,
    /// Maximum size of gossipsub messages in bytes (default: `MAX_PUBSUB_MSG_SIZE`)
    pub max_pubsub_msg_size: usize,
    /// Maximum number of peers to keep in the address cache (default: 1024)
    pub addr_cache_size: NonZeroUsize,
    /// Timeout for worker status requests (default: 15 sec).
    pub status_request_timeout: Duration,
    /// How often to request the status from each individual worker (default: 60 sec).
    pub status_request_frequency: Duration,
    /// How many concurrent status requests to send to the workers (default: 100).
    pub concurrent_status_requests: usize,
    /// Whether to use the gossipsub protocol for collecting worker heartbeats (default: true)
    pub worker_status_via_gossipsub: bool,
}

impl BaseConfig {
    pub fn from_env() -> Self {
        let onchain_update_interval =
            Duration::from_secs(parse_env_var("ONCHAIN_UPDATE_INTERVAL_SEC", 60));
        let autonat_timeout = Duration::from_secs(parse_env_var("AUTONAT_TIMEOUT_SEC", 60));
        let identify_interval = Duration::from_secs(parse_env_var("IDENTIFY_INTERVAL_SEC", 60));
        let probe_timeout = Duration::from_secs(parse_env_var("PROBE_TIMEOUT_SEC", 20));
        let kad_query_timeout = Duration::from_secs(parse_env_var("KAD_QUERY_TIMEOUT_SEC", 5));
        let max_concurrent_probes = parse_env_var("MAX_CONCURRENT_PROBES", 1024);
        let max_pubsub_msg_size = parse_env_var("MAX_PUBSUB_MSG_SIZE", MAX_PUBSUB_MSG_SIZE);
        let addr_cache_size = NonZeroUsize::new(parse_env_var("ADDR_CACHE_SIZE", 1024))
            .expect("addr_cache_size should be > 0");
        let status_request_timeout =
            Duration::from_secs(parse_env_var("WORKER_STATUS_TIMEOUT_SEC", 15));
        let status_request_frequency =
            Duration::from_secs(parse_env_var("WORKER_STATUS_FREQUENCY_SEC", 60));
        let concurrent_status_requests = parse_env_var("WORKER_CONCURRENT_STATUS_UPDATES", 100);
        Self {
            onchain_update_interval,
            autonat_timeout,
            identify_interval,
            probe_timeout,
            kad_query_timeout,
            max_concurrent_probes,
            max_pubsub_msg_size,
            addr_cache_size,
            status_request_timeout,
            status_request_frequency,
            concurrent_status_requests,
            worker_status_via_gossipsub: true,
        }
    }
}

pub struct BaseBehaviour {
    inner: InnerBehaviour,
    config: BaseConfig,
    keypair: Keypair,
    pending_events: VecDeque<TToSwarm<Self>>,
    pending_outbound_conns: BiHashMap<PeerId, ConnectionId>,
    ongoing_queries: BiHashMap<PeerId, QueryId>,
    outbound_conns: HashMap<PeerId, u32>,
    probe_timeouts: FuturesMap<PeerId, ()>,
    registered_workers: Arc<RwLock<HashSet<PeerId>>>,
    heartbeats_stream: Option<Pin<Box<dyn Stream<Item = Option<(Heartbeat, PeerId)>> + Send>>>,
}

#[allow(dead_code)]
impl BaseBehaviour {
    pub fn new(
        keypair: &Keypair,
        contract_client: Box<dyn ContractClient>,
        config: BaseConfig,
        boot_nodes: Vec<BootNode>,
        relay: relay::client::Behaviour,
        dht_protocol: StreamProtocol,
        agent_info: AgentInfo,
    ) -> Self {
        let local_peer_id = keypair.public().to_peer_id();
        let mut kad_config = kad::Config::new(dht_protocol);
        kad_config.set_query_timeout(config.kad_query_timeout);
        let mut inner = InnerBehaviour {
            identify: identify::Behaviour::new(
                identify::Config::new(ID_PROTOCOL.to_string(), keypair.public())
                    .with_interval(config.identify_interval)
                    .with_push_listen_addr_updates(true)
                    .with_agent_version(agent_info.to_string()),
            ),
            kademlia: kad::Behaviour::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                kad_config,
            ),
            relay,
            dcutr: dcutr::Behaviour::new(local_peer_id),
            ping: ping::Behaviour::new(ping::Config::default()),
            autonat: autonat::Behaviour::new(
                local_peer_id,
                autonat::Config {
                    timeout: config.autonat_timeout,
                    use_connected: false,
                    ..Default::default()
                },
            ),
            whitelist: WhitelistBehavior::new(
                contract_client.clone_client(),
                WhitelistConfig::new(config.onchain_update_interval),
            )
            .into(),
            pubsub: PubsubBehaviour::new(keypair.clone(), config.max_pubsub_msg_size).into(),
            address_cache: AddressCache::new(config.addr_cache_size),
            stream: ClientBehaviour::default().into(),
        };

        for boot_node in boot_nodes {
            inner.whitelist.allow_peer(boot_node.peer_id);
            inner.autonat.add_server(boot_node.peer_id, Some(boot_node.address));
        }

        Self {
            inner,
            keypair: keypair.clone(),
            pending_events: Default::default(),
            pending_outbound_conns: Default::default(),
            ongoing_queries: Default::default(),
            outbound_conns: Default::default(),
            probe_timeouts: FuturesMap::new(config.probe_timeout, config.max_concurrent_probes),
            registered_workers: Arc::new(RwLock::new(Default::default())),
            heartbeats_stream: None,
            config,
        }
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    // Prevents removing the address from the DHT even if AutoNAT check fails
    pub fn set_server_mode(&mut self) {
        self.inner.kademlia.set_mode(Some(kad::Mode::Server));
    }

    pub fn request_handle(
        &self,
        protocol: &'static str,
        config: ClientConfig,
    ) -> StreamClientHandle {
        self.inner.stream.new_handle(protocol, config)
    }

    pub fn subscribe_heartbeats(&mut self) {
        if self.config.worker_status_via_gossipsub {
            let registered_workers = self.registered_workers.clone();
            let config = MsgValidationConfig::new(HEARTBEATS_MIN_INTERVAL)
                .max_burst(2)
                .msg_validator(move |peer_id: PeerId, _seq_no: u64, _data: &[u8]| {
                    if !registered_workers.read().contains(&peer_id) {
                        return Err(ValidationError::Invalid("Worker not registered"));
                    }
                    Ok(())
                });
            self.inner.pubsub.subscribe(HEARTBEAT_TOPIC, config);
        }

        let registered_workers = self.registered_workers.clone();
        let status_stream_handle = self.inner.stream.new_handle(
            WORKER_STATUS_PROTOCOL,
            ClientConfig {
                max_concurrent_streams: None,
                max_response_size: MAX_HEARTBEAT_SIZE,
                request_timeout: self.config.status_request_timeout,
                ..Default::default()
            },
        );
        self.heartbeats_stream = Some(Box::pin(stream_heartbeats(
            registered_workers.clone(),
            status_stream_handle,
            self.config.status_request_frequency,
            self.config.concurrent_status_requests,
        )));
    }

    pub fn publish_heartbeat(&mut self, heartbeat: Heartbeat) {
        self.inner.pubsub.publish(HEARTBEAT_TOPIC, heartbeat.encode_to_vec());
        #[cfg(feature = "metrics")]
        HEARTBEATS_PUBLISHED.inc();
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

    pub fn outbound_conn_exists(&self, peer_id: &PeerId) -> bool {
        self.outbound_conns.get(peer_id).is_some_and(|x| *x > 0)
    }

    pub fn can_be_dialed(&self, peer_id: &PeerId) -> bool {
        self.outbound_conn_exists(peer_id) || self.inner.address_cache.contains(peer_id)
    }

    fn try_schedule_probe(&mut self, peer_id: PeerId) -> Result<(), TryProbeError> {
        if self.probe_timeouts.contains(peer_id) {
            log::debug!("Probe for peer {peer_id} already ongoing");
            return Err(TryProbeError::Ongoing);
        }
        if self.probe_timeouts.try_push(peer_id, futures::future::pending()).is_err() {
            log::debug!("Too  many ongoing probes");
            return Err(TryProbeError::TooManyProbes);
        }
        if self.outbound_conn_exists(&peer_id) {
            log::debug!("Closing outbound connection(s) to {peer_id}");
            self.pending_events.push_back(ToSwarm::CloseConnection {
                peer_id,
                connection: Default::default(),
            });
        }
        log::debug!("Probing peer {peer_id}");
        #[cfg(feature = "metrics")]
        ONGOING_PROBES.inc();
        Ok(())
    }

    /// Try to find peer on DHT and connect
    pub fn try_probe_dht(&mut self, peer_id: PeerId) -> Result<(), TryProbeError> {
        self.try_schedule_probe(peer_id)?;
        self.find_and_dial(peer_id);
        Ok(())
    }

    /// Try to connect to peer directly
    pub fn try_probe_direct(
        &mut self,
        peer_id: PeerId,
        addr: Multiaddr,
    ) -> Result<(), TryProbeError> {
        self.try_schedule_probe(peer_id)?;
        let dial_opts = DialOpts::peer_id(peer_id)
            .addresses(vec![addr])
            .condition(PeerCondition::Always)
            .build();
        let conn_id = dial_opts.connection_id();
        self.pending_outbound_conns.insert(peer_id, conn_id);
        self.pending_events.push_back(ToSwarm::Dial { opts: dial_opts });
        Ok(())
    }

    fn on_dial_failure(
        &mut self,
        peer_id: PeerId,
        conn_id: ConnectionId,
        error: String,
    ) -> Option<TToSwarm<Self>> {
        self.pending_outbound_conns.remove_by_right(&conn_id)?;
        log::debug!("Probe for peer {peer_id} failed: {error}");
        #[cfg(feature = "metrics")]
        if ONGOING_PROBES.dec() == 0 {
            ONGOING_PROBES.set(0); // FIXME: There is underflow sometimes
        }
        _ = self.probe_timeouts.remove(peer_id);
        Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::PeerProbed(PeerProbed {
            peer_id,
            result: ProbeResult::Error(error.into_boxed_str()),
        })))
    }

    fn on_probe_timeout(&mut self, peer_id: PeerId) -> TToSwarm<Self> {
        log::debug!("Probe for peer {peer_id} timed out");
        #[cfg(feature = "metrics")]
        if ONGOING_PROBES.dec() == 0 {
            ONGOING_PROBES.set(0); // FIXME: There is underflow sometimes
        }
        self.pending_outbound_conns.remove_by_left(&peer_id);
        ToSwarm::GenerateEvent(BaseBehaviourEvent::PeerProbed(PeerProbed {
            peer_id,
            result: ProbeResult::Timeout,
        }))
    }

    pub fn allow_peer(&mut self, peer_id: PeerId) {
        self.inner.whitelist.allow_peer(peer_id);
    }
}

// TODO: make it only return successful responses
fn stream_heartbeats(
    registered_workers: Arc<RwLock<HashSet<PeerId>>>,
    stream_handle: StreamClientHandle,
    frequency: Duration,
    parallelism: usize,
) -> impl Stream<Item = Option<(Heartbeat, PeerId)>> {
    let mut interval = tokio::time::interval(frequency);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio_stream::wrappers::IntervalStream::new(interval).flat_map(move |_| {
        let workers = registered_workers.read().clone();
        let stream_handle = stream_handle.clone();
        futures::stream::iter(workers.into_iter())
            .map(move |peer_id| {
                let stream_handle = stream_handle.clone();
                async move {
                    let resp = stream_handle
                        .request_response(peer_id, b"")
                        .await
                        .inspect_err(|e| log::debug!("Couldn't send heartbeat request: {e:?}"))
                        .ok()?;
                    let heartbeat = Heartbeat::decode(resp.as_ref())
                        .inspect_err(|e| {
                            log::debug!("Error decoding heartbeat from {peer_id}: {e:?}")
                        })
                        .ok()?;
                    log::debug!("Got heartbeat from {peer_id}: {heartbeat:?}");
                    Some((heartbeat, peer_id))
                }
            })
            .buffer_unordered(parallelism)
    })
}

#[derive(Debug, Clone)]
pub enum BaseBehaviourEvent {
    Heartbeat {
        peer_id: PeerId,
        heartbeat: Heartbeat,
    },
    PeerProbed(PeerProbed),
}

#[derive(Debug, Clone)]
pub struct PeerProbed {
    pub peer_id: PeerId,
    pub result: ProbeResult,
}

#[derive(Debug, Clone)]
pub enum ProbeResult {
    Timeout,
    Error(Box<str>),
    Reachable {
        listen_addrs: Vec<Multiaddr>,
        agent_version: Box<str>,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum TryProbeError {
    #[error("There are too many active probes")]
    TooManyProbes,
    #[error("There is already an ongoing probe for this peer")]
    Ongoing,
}

impl BehaviourWrapper for BaseBehaviour {
    type Inner = InnerBehaviour;
    type Event = BaseBehaviourEvent;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_swarm_event(&mut self, ev: FromSwarm) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            FromSwarm::ConnectionEstablished(conn) => self.on_connection_established(conn),
            FromSwarm::ConnectionClosed(conn) => self.on_connection_closed(conn),
            FromSwarm::DialFailure(DialFailure {
                peer_id: Some(peer_id),
                error,
                connection_id,
            }) => self.on_dial_failure(peer_id, connection_id, error.to_string()),
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
            InnerBehaviourEvent::Ping(ev) => {
                record_event(&ev);
                None
            }
            InnerBehaviourEvent::Dcutr(ev) => {
                record_event(&ev);
                None
            }
            InnerBehaviourEvent::Whitelist(nodes) => self.on_nodes_update(nodes),
            InnerBehaviourEvent::Stream(ev) => self.on_stream_event(ev),
            _ => None,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        if let Some(ev) = self.pending_events.pop_front() {
            return Poll::Ready(Some(ev));
        }

        if let Some(stream) = self.heartbeats_stream.as_mut() {
            if let Poll::Ready(item) = stream.poll_next_unpin(cx) {
                let heartbeat = item.expect("Heartbeat stream should never finish");
                if let Some((heartbeat, peer_id)) = heartbeat {
                    return Poll::Ready(Some(ToSwarm::GenerateEvent(
                        BaseBehaviourEvent::Heartbeat { peer_id, heartbeat },
                    )));
                }
            }
        }

        match self.probe_timeouts.poll_unpin(cx) {
            Poll::Ready((peer_id, Err(_))) => {
                return Poll::Ready(Some(self.on_probe_timeout(peer_id)));
            }
            Poll::Pending => {}
            _ => unreachable!(), // future::pending() should never complete
        }

        return Poll::Pending;
    }
}

impl BaseBehaviour {
    fn on_connection_established(&mut self, conn: ConnectionEstablished) -> Option<TToSwarm<Self>> {
        #[cfg(feature = "metrics")]
        ACTIVE_CONNECTIONS.inc();
        let peer_id = match conn.endpoint {
            ConnectedPoint::Dialer { .. } => conn.peer_id,
            _ => return None,
        };
        log::debug!("Established outbound connection to {peer_id}");
        *self.outbound_conns.entry(peer_id).or_default() += 1;
        None
    }

    fn on_connection_closed(&mut self, conn: ConnectionClosed) -> Option<TToSwarm<Self>> {
        #[cfg(feature = "metrics")]
        ACTIVE_CONNECTIONS.dec();
        let peer_id = match conn.endpoint {
            ConnectedPoint::Dialer { .. } => conn.peer_id,
            _ => return None,
        };
        log::debug!("Closed outbound connection to {peer_id}");
        match self.outbound_conns.get_mut(&peer_id) {
            Some(x) => *x -= 1,
            None => log::error!("Closed connection not established before"),
        }
        None
    }

    fn on_identify_event(&mut self, ev: identify::Event) -> Option<TToSwarm<Self>> {
        log::debug!("Identify event received: {ev:?}");
        record_event(&ev);
        let (peer_id, listen_addrs, agent_version, conn_id) = match ev {
            identify::Event::Received {
                peer_id,
                info,
                connection_id,
            } => (peer_id, info.listen_addrs, info.agent_version, connection_id),
            _ => return None,
        };

        // Filter out unreachable (private) addresses and add the remaining to cache and DHT
        let listen_addrs = listen_addrs.into_iter().filter(addr_is_reachable);
        self.inner.address_cache.put(peer_id, listen_addrs.clone());
        listen_addrs.clone().for_each(|addr| {
            self.inner.kademlia.add_address(&peer_id, addr);
        });

        let pending_conn = self.pending_outbound_conns.get_by_left(&peer_id);
        // In case of a DHT probe, there should be no connection ID in `pending_outbound_conns`
        // In case of a direct probe, the connection ID should match the one stored
        if self.probe_timeouts.contains(peer_id)
            && (pending_conn.is_none() || pending_conn.is_some_and(|id| id == &conn_id))
        {
            self.probe_timeouts.remove(peer_id);
            self.pending_outbound_conns.remove_by_left(&peer_id);
            #[cfg(feature = "metrics")]
            if ONGOING_PROBES.dec() == 0 {
                ONGOING_PROBES.set(0); // FIXME: There is underflow sometimes
            }
            log::debug!("Probe for {peer_id} succeeded");
            Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::PeerProbed(PeerProbed {
                peer_id,
                result: ProbeResult::Reachable {
                    listen_addrs: listen_addrs.collect(),
                    agent_version: agent_version.into_boxed_str(),
                },
            })))
        } else {
            None
        }
    }

    fn on_kademlia_event(&mut self, ev: kad::Event) -> Option<TToSwarm<Self>> {
        log::debug!("Kademlia event received: {ev:?}");
        record_event(&ev);
        let kad::Event::OutboundQueryProgressed {
            id: query_id,
            result: QueryResult::GetClosestPeers(result),
            step: ProgressStep { last, .. },
            ..
        } = ev
        else {
            match ev {
                kad::Event::RoutablePeer { peer, address }
                | kad::Event::PendingRoutablePeer { peer, address } => {
                    self.inner.address_cache.put(peer, Some(address));
                }
                _ => {}
            }
            return None;
        };

        let peer_id = self.ongoing_queries.get_by_right(&query_id)?.to_owned();
        let peer_info = match result {
            Ok(GetClosestPeersOk { peers, .. })
            | Err(GetClosestPeersError::Timeout { peers, .. }) => {
                peers.into_iter().find(|p| p.peer_id == peer_id)
            }
        };
        let query_finished = last || peer_info.is_some();

        // Query finished
        if query_finished {
            log::debug!("Query for peer {peer_id} finished.");
            self.ongoing_queries.remove_by_right(&query_id);
            #[cfg(feature = "metrics")]
            ONGOING_QUERIES.dec();
        }

        if let Some(peer_info) = peer_info {
            // Cache the found address(es) so they can be used for dialing
            // (kademlia might not do it by itself, if the bucket is full)
            self.inner.address_cache.put(peer_id, peer_info.addrs);
        }

        // Try to dial even if `peer_info` is `None`.
        // There might be some address(es) cached from previous queries.
        query_finished.then_some(ToSwarm::Dial {
            // Not using the default condition (`DisconnectedAndNotDialing`), because we may want
            // to establish an outbound connection to the peer despite existing inbound connection.
            opts: DialOpts::peer_id(peer_id).condition(PeerCondition::NotDialing).build(),
        })
    }

    fn on_autonat_event(&mut self, ev: autonat::Event) -> Option<TToSwarm<Self>> {
        log::debug!("AutoNAT event received: {ev:?}");
        let autonat::Event::StatusChanged { new: status, .. } = ev else {
            return None;
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
        log::trace!("Pub-sub message received: peer_id={peer_id} topic={topic}");
        let data = data.as_ref();
        let ev = match topic {
            HEARTBEAT_TOPIC => decode_heartbeat(peer_id, data)?,
            _ => return None,
        };
        Some(ToSwarm::GenerateEvent(ev))
    }

    // TODO: consider capturing all dial requests, not only from the stream behaviour
    fn on_stream_event(&mut self, ev: stream_client::Event) -> Option<TToSwarm<Self>> {
        // When trying to dial an unknown peer, try to find it on DHT first
        let stream_client::Event::Dial(opts) = ev;
        match opts.get_peer_id() {
            Some(peer_id) if !self.can_be_dialed(&peer_id) => {
                // The ConnectionId will not correspond to the requested one,
                // but it's not used by the stream behaviour anyway
                self.find_and_dial(peer_id);
                None
            }
            _ => Some(ToSwarm::Dial { opts }),
        }
    }

    fn on_nodes_update(&mut self, nodes: NetworkNodes) -> Option<TToSwarm<Self>> {
        log::debug!("Updating registered workers");
        *self.registered_workers.write() = nodes.workers;
        None
    }
}

fn decode_heartbeat(peer_id: PeerId, data: &[u8]) -> Option<BaseBehaviourEvent> {
    let heartbeat = Heartbeat::decode(data)
        .map_err(|e| log::warn!("Error decoding heartbeat: {e:?}"))
        .ok()?;
    #[cfg(feature = "metrics")]
    HEARTBEATS_RECEIVED.inc();
    Some(BaseBehaviourEvent::Heartbeat { peer_id, heartbeat })
}
