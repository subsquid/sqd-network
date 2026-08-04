use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::NonZeroUsize,
    task::{Context, Poll},
    time::Duration,
    vec,
};

use bimap::BiHashMap;
use libp2p::{
    autonat::{self, NatStatus},
    core::ConnectedPoint,
    identify,
    identity::Keypair,
    kad::{
        self, store::MemoryStore, GetClosestPeersError, GetClosestPeersOk, GetProvidersError,
        GetProvidersOk, ProgressStep, QueryId, QueryResult, QueryStats,
    },
    ping,
    swarm::{
        behaviour::ConnectionEstablished,
        dial_opts::{DialOpts, PeerCondition},
        ConnectionClosed, DialError, DialFailure, FromSwarm, NetworkBehaviour, ToSwarm,
    },
    StreamProtocol,
};
use libp2p_swarm_derive::NetworkBehaviour;
use serde::{Deserialize, Serialize};
use tokio_util::time::DelayQueue;

use sqd_contract_client::{Client as ContractClient, NetworkNodes};

#[cfg(feature = "metrics")]
use crate::metrics::{ACTIVE_CONNECTIONS, ONGOING_LOOKUPS};
use crate::{
    behaviour::{
        addr_cache::AddressCache,
        keep_alive::KeepAliveBehaviour,
        node_whitelist::{WhitelistBehavior, WhitelistConfig},
        wrapped::{BehaviourWrapper, TToSwarm, Wrapped},
    },
    cli::BootNode,
    protocol::ID_PROTOCOL,
    record_event,
    util::{addr_is_reachable, parse_env_var},
    AgentInfo, PeerId,
};
#[cfg(feature = "pubsub")]
use crate::{protocol::MAX_PUBSUB_MSG_SIZE, PubsubBehaviour, PubsubMsg};

use super::stream_client::{ClientBehaviour, ClientConfig, StreamClientHandle};

#[derive(NetworkBehaviour)]
pub struct InnerBehaviour {
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    whitelist: Wrapped<WhitelistBehavior>,
    #[cfg(feature = "pubsub")]
    pubsub: Wrapped<PubsubBehaviour>,
    address_cache: AddressCache,
    stream: Wrapped<ClientBehaviour>,
    keep_alive: KeepAliveBehaviour,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BaseConfig {
    /// How often to check for on-chain updates: current epoch and registered nodes (default: 3 min).
    pub onchain_update_interval: Duration,
    /// Timeout for autoNAT probes (default: 60 sec).
    pub autonat_timeout: Duration,
    /// How often to publish identify info to connected nodes (default: 10 min).
    pub identify_interval: Duration,
    /// Timeout for kademlia DHT queries (default: 5 sec).
    pub kad_query_timeout: Duration,
    /// Maximum size of gossipsub messages in bytes (default: `MAX_PUBSUB_MSG_SIZE`)
    #[cfg(feature = "pubsub")]
    pub max_pubsub_msg_size: usize,
    /// Maximum number of peers to keep in the address cache (default: 1024)
    pub addr_cache_size: NonZeroUsize,
    /// Maximum number of concurrent Kademlia lookups for worker discovery (default: 20)
    pub max_concurrent_lookups: usize,
    /// Cooldown before re-dialing a worker after its connection closes (default: 30 sec).
    /// Prevents a tight dial→refuse→redial loop against workers that reject this peer.
    pub reconnect_cooldown: Duration,
}

impl BaseConfig {
    pub fn from_env() -> Self {
        let onchain_update_interval =
            Duration::from_secs(parse_env_var("ONCHAIN_UPDATE_INTERVAL_SEC", 60));
        let autonat_timeout = Duration::from_secs(parse_env_var("AUTONAT_TIMEOUT_SEC", 60));
        let identify_interval = Duration::from_secs(parse_env_var("IDENTIFY_INTERVAL_SEC", 600));
        let kad_query_timeout = Duration::from_secs(parse_env_var("KAD_QUERY_TIMEOUT_SEC", 5));
        #[cfg(feature = "pubsub")]
        let max_pubsub_msg_size = parse_env_var("MAX_PUBSUB_MSG_SIZE", MAX_PUBSUB_MSG_SIZE);
        let addr_cache_size = NonZeroUsize::new(parse_env_var("ADDR_CACHE_SIZE", 1024))
            .expect("addr_cache_size should be > 0");
        let max_concurrent_lookups = parse_env_var("MAX_CONCURRENT_LOOKUPS", 20);
        let reconnect_cooldown = Duration::from_secs(parse_env_var("RECONNECT_COOLDOWN_SEC", 30));
        Self {
            onchain_update_interval,
            autonat_timeout,
            identify_interval,
            kad_query_timeout,
            #[cfg(feature = "pubsub")]
            max_pubsub_msg_size,
            addr_cache_size,
            max_concurrent_lookups,
            reconnect_cooldown,
        }
    }
}

pub struct BaseBehaviour {
    inner: InnerBehaviour,
    keypair: Keypair,
    pending_events: VecDeque<TToSwarm<Self>>,
    ongoing_lookups: BiHashMap<PeerId, QueryId>,
    outbound_conns: HashMap<PeerId, u32>,
    registered_workers: HashSet<PeerId>,
    whitelist_initialized: bool,
    identified_peers: HashSet<PeerId>,
    /// Last confidence value sent, in tenths (0–10). None means nothing sent yet.
    last_sent_confidence_step: Option<u8>,
    maintain_worker_connections: bool,
    pending_lookups: VecDeque<PeerId>,
    max_concurrent_lookups: usize,
    /// Flat cooldown applied before re-dialing a worker whose connection just closed.
    reconnect_cooldown: Duration,
    /// Workers awaiting a cooldown before reconnection.
    reconnect_queue: DelayQueue<PeerId>,
    /// Peers currently present in `reconnect_queue`, to avoid scheduling duplicates.
    reconnect_scheduled: HashSet<PeerId>,
}

#[allow(dead_code)]
impl BaseBehaviour {
    pub fn new(
        keypair: &Keypair,
        contract_client: Box<dyn ContractClient>,
        config: BaseConfig,
        boot_nodes: Vec<BootNode>,
        dht_protocol: StreamProtocol,
        agent_info: AgentInfo,
    ) -> Self {
        let local_peer_id = keypair.public().to_peer_id();
        let mut kad_config = kad::Config::new(dht_protocol);
        kad_config.set_query_timeout(config.kad_query_timeout);
        kad_config.set_replication_interval(Some(Duration::from_secs(10 * 60)));
        kad_config.set_publication_interval(Some(Duration::from_secs(60 * 60)));
        kad_config.set_provider_publication_interval(Some(Duration::from_secs(10 * 60)));
        let only_global_ips = std::env::var("PRIVATE_NETWORK").is_err();

        let mut inner = InnerBehaviour {
            identify: identify::Behaviour::new(
                identify::Config::new(ID_PROTOCOL.to_string(), keypair.public())
                    .with_interval(config.identify_interval)
                    .with_cache_size(0)
                    .with_push_listen_addr_updates(true)
                    .with_agent_version(agent_info.to_string()),
            ),
            kademlia: kad::Behaviour::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                kad_config,
            ),
            ping: ping::Behaviour::new(ping::Config::default()),
            autonat: autonat::Behaviour::new(
                local_peer_id,
                autonat::Config {
                    timeout: config.autonat_timeout,
                    use_connected: false,
                    only_global_ips,
                    ..Default::default()
                },
            ),
            whitelist: WhitelistBehavior::new(
                contract_client,
                WhitelistConfig::new(config.onchain_update_interval),
            )
            .into(),
            #[cfg(feature = "pubsub")]
            pubsub: PubsubBehaviour::new(keypair.clone(), config.max_pubsub_msg_size).into(),
            address_cache: AddressCache::new(config.addr_cache_size),
            stream: ClientBehaviour::default().into(),
            keep_alive: KeepAliveBehaviour::default(),
        };

        for boot_node in boot_nodes {
            inner.whitelist.allow_peer(boot_node.peer_id);
            inner.kademlia.add_address(&boot_node.peer_id, boot_node.address.clone());
            inner.autonat.add_server(boot_node.peer_id, Some(boot_node.address));
        }

        Self {
            inner,
            keypair: keypair.clone(),
            pending_events: Default::default(),
            ongoing_lookups: Default::default(),
            outbound_conns: Default::default(),
            registered_workers: Default::default(),
            whitelist_initialized: false,
            identified_peers: HashSet::new(),
            last_sent_confidence_step: None,
            maintain_worker_connections: false,
            pending_lookups: Default::default(),
            max_concurrent_lookups: config.max_concurrent_lookups,
            reconnect_cooldown: config.reconnect_cooldown,
            reconnect_queue: DelayQueue::new(),
            reconnect_scheduled: HashSet::new(),
        }
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    // Prevents removing the address from the DHT even if AutoNAT check fails
    pub fn set_server_mode(&mut self) {
        self.inner.kademlia.set_mode(Some(kad::Mode::Server));
    }

    pub fn maintain_worker_connections(&mut self) {
        self.inner.keep_alive.keep_all_connections_alive();
        self.maintain_worker_connections = true;
    }

    pub fn request_handle(
        &self,
        protocol: &'static str,
        config: ClientConfig,
    ) -> StreamClientHandle {
        self.inner.stream.new_handle(protocol, config)
    }

    pub fn get_kademlia_mut(&mut self) -> &mut kad::Behaviour<MemoryStore> {
        &mut self.inner.kademlia
    }

    pub fn find_and_dial(&mut self, peer_id: PeerId) {
        if self.inner.address_cache.contains(&peer_id) {
            log::debug!("Dialing peer {peer_id} using cached address");
            self.pending_events.push_back(ToSwarm::Dial {
                opts: DialOpts::peer_id(peer_id).build(),
            });
        } else if self.ongoing_lookups.contains_left(&peer_id) {
            log::debug!("Query for peer {peer_id} already ongoing");
        } else {
            log::debug!("Starting query for peer {peer_id}");
            let query_id = self.inner.kademlia.get_closest_peers(peer_id);
            self.ongoing_lookups.insert(peer_id, query_id);
            #[cfg(feature = "metrics")]
            ONGOING_LOOKUPS.inc();
        }
    }

    pub fn outbound_conn_exists(&self, peer_id: &PeerId) -> bool {
        self.outbound_conns.get(peer_id).is_some_and(|x| *x > 0)
    }

    pub fn allow_peer(&mut self, peer_id: PeerId) {
        self.inner.whitelist.allow_peer(peer_id);
    }
}

#[derive(Debug, Clone)]
pub enum BaseBehaviourEvent {
    ProviderRecord {
        id: QueryId,
        result: Result<GetProvidersOk, GetProvidersError>,
        stats: QueryStats,
        step: ProgressStep,
    },
    NetworkConnected {
        /// Connection confidence in range [0.1, 1.0], sent in steps of 0.1.
        /// Reaches 1.0 when identified peers ≥ 75% of whitelist size.
        confidence: f32,
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
            FromSwarm::ConnectionEstablished(conn) => self.on_connection_established(conn),
            FromSwarm::ConnectionClosed(conn) => self.on_connection_closed(conn),
            FromSwarm::DialFailure(failure) => self.on_dial_failure(failure),
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
            #[cfg(feature = "pubsub")]
            InnerBehaviourEvent::Pubsub(ev) => self.on_pubsub_event(ev),
            InnerBehaviourEvent::Ping(ev) => {
                record_event(&ev);
                None
            }
            InnerBehaviourEvent::Whitelist(nodes) => self.on_nodes_update(nodes),
            _ => None,
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        // Drain expired reconnects first, unconditionally: `poll_expired` is what registers
        // the timer waker, so it must run even when there's a pending event to emit. Skipping
        // it behind the early return below would let a cooldown entry expire without a
        // registered waker, deferring the redial until some unrelated event wakes the task.
        // Known limitation: `reconnect_scheduled` is cleared here rather than when the dial is
        // actually issued, so it de-duplicates against `reconnect_queue` but not against
        // `pending_lookups`. A peer held back by the `max_concurrent_lookups` cap therefore
        // collects one extra `pending_lookups` entry per dial failure, and that queue is
        // unbounded. Harmless at the current worker counts, where the cap is not reached;
        // tracking the dial through to its start would need more state than it is worth.
        while let Poll::Ready(Some(expired)) = self.reconnect_queue.poll_expired(cx) {
            let peer_id = expired.into_inner();
            self.reconnect_scheduled.remove(&peer_id);
            if !self.outbound_conn_exists(&peer_id) && self.registered_workers.contains(&peer_id) {
                self.pending_lookups.push_back(peer_id);
            }
        }

        while self.ongoing_lookups.len() < self.max_concurrent_lookups {
            let Some(peer_id) = self.pending_lookups.pop_front() else {
                break;
            };
            if !self.outbound_conn_exists(&peer_id) {
                // find_and_dial inserts into ongoing_lookups only for actual DHT queries;
                // cached-address dials don't count toward the max_concurrent_lookups cap.
                self.find_and_dial(peer_id);
            }
        }

        if let Some(ev) = self.pending_events.pop_front() {
            return Poll::Ready(Some(ev));
        }

        Poll::Pending
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
        if self.maintain_worker_connections
            && !self.outbound_conn_exists(&peer_id)
            && self.registered_workers.contains(&peer_id)
            // Skip if a reconnect is already pending (avoids a tight redial loop and
            // unbounded duplicate entries for workers that keep refusing this peer).
            && self.reconnect_scheduled.insert(peer_id)
        {
            log::debug!(
                "Worker {peer_id} disconnected, scheduling reconnection in {:?}",
                self.reconnect_cooldown
            );
            self.reconnect_queue.insert(peer_id, self.reconnect_cooldown);
        }
        None
    }

    /// A failed dial to a registered worker is the only signal that its cached address is dead:
    /// `AddressCache` drops the entry on exactly these errors (`addr_cache.rs`), and until this
    /// arm existed nothing re-armed afterwards, because a failed dial establishes no connection
    /// and so produces no close event. The worker then stayed undialable until unrelated DHT
    /// traffic happened to supply an address — the stranding in NET-384.
    ///
    /// `DialError::NoAddresses` is deliberately not covered. It also evicts, but it is the error
    /// a fruitless lookup produces, so scheduling on it would let the recovery feed itself
    /// indefinitely.
    ///
    /// Retries are unbounded: a worker that stays registered is one we are supposed to hold a
    /// connection to, so it is worth looking up for as long as that holds. `reconnect_scheduled`
    /// and the cooldown cap the cost at one lookup per peer per `reconnect_cooldown`, and the
    /// registered set is what bounds the population.
    ///
    /// The queued dial goes through `find_and_dial`, so it uses the address cache if something
    /// refilled it during the cooldown. That is the right call when the new address is fresher
    /// than the failure — a concurrent lookup landing its result — and costs one extra cooldown
    /// when it is Kademlia echoing back the address we just disproved, after which the next
    /// failure re-arms. Unbounded retries are what make that merely slow instead of terminal.
    fn on_dial_failure(&mut self, failure: DialFailure) -> Option<TToSwarm<Self>> {
        let peer_id = failure.peer_id?;
        if !matches!(failure.error, DialError::Transport(_) | DialError::WrongPeerId { .. }) {
            return None;
        }
        if !self.maintain_worker_connections
            || self.outbound_conn_exists(&peer_id)
            || !self.registered_workers.contains(&peer_id)
        {
            return None;
        }
        // Skip if a reconnect is already pending, as in `on_connection_closed`.
        if !self.reconnect_scheduled.insert(peer_id) {
            return None;
        }
        log::debug!(
            "Dial to worker {peer_id} failed, scheduling a lookup in {:?}",
            self.reconnect_cooldown
        );
        self.reconnect_queue.insert(peer_id, self.reconnect_cooldown);
        None
    }

    fn on_identify_event(&mut self, ev: identify::Event) -> Option<TToSwarm<Self>> {
        log::debug!("Identify event received: {ev:?}");
        record_event(&ev);
        let (peer_id, listen_addrs) = match ev {
            identify::Event::Received { peer_id, info, .. } => (peer_id, info.listen_addrs),
            _ => return None,
        };

        // Filter out unreachable (private) addresses and add the remaining to cache and DHT
        let listen_addrs = listen_addrs.into_iter().filter(addr_is_reachable);
        self.inner.address_cache.put(peer_id, listen_addrs.clone());
        listen_addrs.clone().for_each(|addr| {
            self.inner.kademlia.add_address(&peer_id, addr);
        });

        self.identified_peers.insert(peer_id);
        self.try_emit_confidence()
    }

    /// Compute current confidence and emit a `NetworkConnected` event if a new 0.1-step
    /// threshold has been crossed for the first time. Returns `None` if the whitelist is
    /// not yet initialised, if the computed step has already been sent, or if the step
    /// would be 0 (first message must carry confidence ≥ 0.1).
    fn try_emit_confidence(&mut self) -> Option<TToSwarm<Self>> {
        if !self.whitelist_initialized {
            return None;
        }
        let whitelist_size = self.inner.whitelist.whitelist_size();
        if whitelist_size == 0 {
            return None;
        }

        // threshold = 75% of whitelist peers
        let threshold = 0.75_f32 * whitelist_size as f32;
        let raw = (self.identified_peers.len() as f32 / threshold).min(1.0_f32);

        // Round down to the nearest 0.1 step (1–10 in tenths).
        // `raw` is clamped to [0.0, 1.0] above, so `raw * 10.0 ∈ [0.0, 10.0]` fits in u8.
        #[allow(clippy::cast_possible_truncation)]
        let step = (raw * 10.0_f32).floor() as u8;

        // First message must have confidence >= 0.1 (step >= 1)
        if step == 0 {
            return None;
        }

        // Only emit when confidence increases; never send the same or lower value twice
        if self.last_sent_confidence_step.is_some_and(|last| step <= last) {
            return None;
        }

        self.last_sent_confidence_step = Some(step);
        let confidence = step as f32 / 10.0_f32;
        log::debug!("Network confidence updated: {confidence:.1}");
        Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::NetworkConnected { confidence }))
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
                libp2p::kad::Event::RoutingUpdated {
                    peer, addresses, ..
                } => {
                    for address in addresses.into_vec() {
                        self.inner.address_cache.put(peer, Some(address));
                    }
                }
                kad::Event::OutboundQueryProgressed {
                    id,
                    result: QueryResult::GetProviders(result),
                    stats,
                    step,
                } => {
                    return Some(ToSwarm::GenerateEvent(BaseBehaviourEvent::ProviderRecord {
                        id,
                        result,
                        stats,
                        step,
                    }));
                }
                _ => {}
            }
            return None;
        };

        let peer_id = self.ongoing_lookups.get_by_right(&query_id)?.to_owned();
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
            self.ongoing_lookups.remove_by_right(&query_id);
            #[cfg(feature = "metrics")]
            ONGOING_LOOKUPS.dec();
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

    #[cfg(feature = "pubsub")]
    fn on_pubsub_event(
        &mut self,
        PubsubMsg { peer_id, topic, .. }: PubsubMsg,
    ) -> Option<TToSwarm<Self>> {
        log::trace!("Pub-sub message received: peer_id={peer_id} topic={topic}");
        None
    }

    fn on_nodes_update(&mut self, nodes: NetworkNodes) -> Option<TToSwarm<Self>> {
        log::debug!("Updating registered workers");
        self.registered_workers = nodes.workers;

        if !self.whitelist_initialized {
            self.whitelist_initialized = true;
            let ongoing = self
                .get_kademlia_mut()
                .iter_queries()
                .any(|q| matches!(q.info(), kad::QueryInfo::Bootstrap { .. }));
            if !ongoing {
                log::debug!("Whitelist initialized, running Kademlia bootstrap");
                if let Err(kad::NoKnownPeers()) = self.get_kademlia_mut().bootstrap() {
                    log::warn!("Failed to trigger bootstrap: no known peers");
                }
            }
        }

        if self.maintain_worker_connections {
            for peer_id in &self.registered_workers {
                if !self.outbound_conn_exists(peer_id)
                    && !self.reconnect_scheduled.contains(peer_id)
                {
                    self.pending_lookups.push_back(*peer_id);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use libp2p::{
        core::{transport::PortUse, Endpoint},
        swarm::ConnectionId,
        Multiaddr, TransportError,
    };
    use sqd_contract_client::{DummyClient, DummyData};

    use super::*;

    fn addr(port: u16, peer: PeerId) -> Multiaddr {
        format!("/ip4/1.2.3.4/udp/{port}/quic-v1/p2p/{peer}").parse().unwrap()
    }

    /// Async because `WhitelistBehavior::new` builds a `tokio::time::interval` stream, which
    /// panics without a reactor.
    fn test_behaviour(keypair: &Keypair) -> BaseBehaviour {
        let config = BaseConfig {
            // Pinned rather than read from the environment: `ONCHAIN_UPDATE_INTERVAL_SEC=0` or
            // `ADDR_CACHE_SIZE=0` in a developer's shell would otherwise panic the test.
            onchain_update_interval: Duration::from_secs(60),
            addr_cache_size: NonZeroUsize::new(16).expect("non-zero"),
            ..BaseConfig::from_env()
        };
        BaseBehaviour::new(
            keypair,
            Box::new(DummyClient::new(DummyData::default())),
            config,
            vec![],
            StreamProtocol::new("/subsquid/dht/test/1.0.0"),
            AgentInfo {
                name: "test",
                version: "0.0.0",
            },
        )
    }

    /// `keypair` must be the *remote* peer's, so `public_key` and `peer_id` agree as they do on
    /// the wire.
    fn identify_received(keypair: &Keypair, listen_addrs: Vec<Multiaddr>) -> identify::Event {
        identify::Event::Received {
            connection_id: ConnectionId::new_unchecked(1),
            peer_id: keypair.public().to_peer_id(),
            info: identify::Info {
                public_key: keypair.public(),
                protocol_version: ID_PROTOCOL.to_owned(),
                agent_version: "sqd-worker/2.13.0".to_owned(),
                listen_addrs,
                protocols: vec![],
                observed_addr: "/ip4/5.6.7.8/udp/1/quic-v1".parse().expect("valid addr"),
                signed_peer_record: None,
            },
        }
    }

    /// A worker behind NAT announces a public address that no longer reaches it, while we are
    /// connected on a different address learned from the DHT. The announced set must not be
    /// treated as authoritative: the address we are demonstrably connected on has to survive in
    /// the routing stores, or the worker becomes permanently undialable (NET-384).
    ///
    /// Asserted against the address cache and the Kademlia routing table individually rather than
    /// against the aggregate of `InnerBehaviour`, because `autonat` keeps its own transient copy
    /// of a connected peer's address and would mask the defect.
    #[tokio::test]
    async fn keeps_confirmed_address_when_peer_announces_a_stale_one() {
        let local = Keypair::generate_ed25519();
        let remote = Keypair::generate_ed25519();
        let peer = remote.public().to_peer_id();
        let working = addr(12208, peer); // the address the connection actually runs on
        let announced = addr(9999, peer); // the stale address the worker advertises

        let mut behaviour = test_behaviour(&local);
        // Stand in for the on-chain registration that normally whitelists a worker.
        behaviour.allow_peer(peer);

        // The DHT lookup that found this worker is what put `working` in the routing table.
        behaviour.inner.kademlia.add_address(&peer, working.clone());

        // The dial succeeded on `working`, so it is confirmed reachable.
        behaviour
            .inner()
            .handle_established_outbound_connection(
                ConnectionId::new_unchecked(1),
                peer,
                &working,
                Endpoint::Dialer,
                PortUse::Reuse,
            )
            .expect("connection should be accepted");

        // Identify arrives over that same connection, announcing only the stale address.
        behaviour.on_identify_event(identify_received(&remote, vec![announced.clone()]));

        let cached = behaviour
            .inner
            .address_cache
            .handle_pending_outbound_connection(
                ConnectionId::new_unchecked(2),
                Some(peer),
                &[],
                Endpoint::Dialer,
            )
            .expect("dial should be allowed");
        let routed = behaviour
            .inner
            .kademlia
            .handle_pending_outbound_connection(
                ConnectionId::new_unchecked(3),
                Some(peer),
                &[],
                Endpoint::Dialer,
            )
            .expect("dial should be allowed");

        // Guards against the test passing vacuously if the identify event stops being applied.
        assert!(
            routed.contains(&announced),
            "identify was not applied: kademlia never learned the announced address"
        );

        assert!(
            cached.contains(&working),
            "address cache dropped the address we are connected on; it would dial {cached:?}"
        );
        assert!(
            routed.contains(&working),
            "kademlia dropped the address we are connected on; it would dial {routed:?}"
        );
    }

    /// Drive the wrapper's own `poll` until it yields nothing more, collecting what it emits.
    /// Runs the reconnect timer and the lookup queue, which is where redials are scheduled.
    fn drain_poll(behaviour: &mut BaseBehaviour) -> Vec<TToSwarm<BaseBehaviour>> {
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let mut events = vec![];
        while let Poll::Ready(evs) = BehaviourWrapper::poll(behaviour, &mut cx) {
            let before = events.len();
            events.extend(evs);
            if events.len() == before {
                break;
            }
        }
        events
    }

    fn dial_failure(peer: PeerId, error: &DialError) -> FromSwarm<'_> {
        FromSwarm::DialFailure(DialFailure {
            peer_id: Some(peer),
            error,
            connection_id: ConnectionId::new_unchecked(2),
        })
    }

    fn dialed(addr: &Multiaddr) -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address: addr.clone(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    }

    fn transport_error(addr: &Multiaddr) -> DialError {
        DialError::Transport(vec![(
            addr.clone(),
            TransportError::Other(std::io::Error::other("host unreachable")),
        )])
    }

    /// Deliver a dial failure the way `Wrapped` does: inner behaviours first (so the address
    /// cache evicts), then the wrapper (see `wrapped.rs`).
    fn deliver_dial_failure(behaviour: &mut BaseBehaviour, peer: PeerId, error: &DialError) {
        behaviour.inner.address_cache.on_swarm_event(dial_failure(peer, error));
        behaviour.on_swarm_event(dial_failure(peer, error));
    }

    /// A registered worker whose connection has closed and whose cached address then failed.
    /// Leaves the behaviour at the point where recovery has to happen.
    fn stranded_worker(local: &Keypair, peer: PeerId, addr: &Multiaddr) -> BaseBehaviour {
        let mut behaviour = test_behaviour(local);
        behaviour.maintain_worker_connections();
        behaviour.on_nodes_update(NetworkNodes {
            portals: HashSet::new(),
            workers: HashSet::from([peer]),
        });
        behaviour.inner.address_cache.put(peer, Some(addr.clone()));
        // The whitelist sweep queues its own dial for a newly registered worker. Let it run, so
        // the state left behind is a plain established connection rather than a half-drained
        // sweep, and `reconnect_scheduled` reflects only what the close below schedules.
        drain_poll(&mut behaviour);

        let endpoint = dialed(addr);
        behaviour.on_connection_established(ConnectionEstablished {
            peer_id: peer,
            connection_id: ConnectionId::new_unchecked(1),
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        });
        behaviour.on_connection_closed(ConnectionClosed {
            peer_id: peer,
            connection_id: ConnectionId::new_unchecked(1),
            endpoint: &endpoint,
            cause: None,
            remaining_established: 0,
        });
        behaviour
    }

    /// A registered worker we are connected to stops being reachable — its host drops off the
    /// network, keeping the peer ID and losing the address.
    ///
    /// The reconnect path fires exactly once, on `ConnectionClosed`, and at that moment the
    /// address cache still holds the now-dead address, so `find_and_dial` dials it instead of
    /// looking the worker up. That dial fails, which empties the cache — and nothing schedules a
    /// Kademlia lookup afterwards, because `reconnect_queue` is fed only by connection closures
    /// and the whitelist sweep is suppressed while the registered set is unchanged. Every later
    /// heartbeat request then dies with `NoAddresses` until unrelated DHT traffic happens to
    /// repopulate the cache, which is the 15–100 minute stranding seen in production (NET-384).
    #[tokio::test(start_paused = true)]
    async fn looks_up_worker_whose_cached_address_stopped_working() {
        let local = Keypair::generate_ed25519();
        let remote = Keypair::generate_ed25519();
        let peer = remote.public().to_peer_id();
        let stale = addr(12208, peer);

        // Registered on chain, connected on a cached address, then dropped off the network.
        let mut behaviour = stranded_worker(&local, peer, &stale);

        // Let the reconnect cooldown elapse, so the queued redial is attempted.
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        let redial = drain_poll(&mut behaviour);
        assert!(
            redial.iter().any(|ev| matches!(ev, ToSwarm::Dial { .. })),
            "expected a redial after the cooldown, got {} event(s)",
            redial.len()
        );

        // That redial goes to the dead address and fails, which empties the address cache.
        deliver_dial_failure(&mut behaviour, peer, &transport_error(&stale));

        // Guards against the test passing vacuously if eviction stops happening on dial failure.
        assert!(
            !behaviour.inner.address_cache.contains(&peer),
            "dial failure no longer evicts; this test would assert nothing"
        );

        // We now know no address for a worker we are supposed to stay connected to. The only way
        // back is a DHT lookup, so one has to be scheduled.
        tokio::time::advance(behaviour.reconnect_cooldown * 2).await;
        drain_poll(&mut behaviour);
        assert!(
            behaviour.ongoing_lookups.contains_left(&peer),
            "no Kademlia lookup started after the cache was emptied: the worker stays undialable \
             until unrelated DHT traffic supplies an address"
        );
    }

    /// If something refills the cache during the cooldown, the queued dial uses it rather than
    /// looking up. When that address is Kademlia echoing back the one we just disproved, the
    /// round is wasted — but the dial fails again and re-arms, so recovery costs an extra
    /// cooldown instead of stranding. This is what lets the change carry no per-peer mode state.
    #[tokio::test(start_paused = true)]
    async fn a_cache_refilled_during_the_cooldown_costs_a_round_but_re_arms() {
        let local = Keypair::generate_ed25519();
        let peer = Keypair::generate_ed25519().public().to_peer_id();
        let stale = addr(12208, peer);

        let mut behaviour = stranded_worker(&local, peer, &stale);
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        drain_poll(&mut behaviour);
        deliver_dial_failure(&mut behaviour, peer, &transport_error(&stale));

        // Kademlia echoes the dead address back while the reconnect waits out its cooldown.
        behaviour.on_kademlia_event(kad::Event::RoutablePeer {
            peer,
            address: stale.clone(),
        });
        assert!(
            behaviour.inner.address_cache.contains(&peer),
            "kademlia did not repopulate the cache; this test would assert nothing"
        );

        // The wasted round: the cached address is dialed rather than looked up.
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        let events = drain_poll(&mut behaviour);
        assert!(
            !behaviour.ongoing_lookups.contains_left(&peer),
            "expected the refilled cache to be dialed, not bypassed"
        );
        assert!(
            events.iter().any(|ev| matches!(ev, ToSwarm::Dial { .. })),
            "expected a dial to the cached address"
        );

        // It fails again, and that re-arms — which is what keeps the round merely wasted.
        deliver_dial_failure(&mut behaviour, peer, &transport_error(&stale));
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        drain_poll(&mut behaviour);
        assert!(
            behaviour.ongoing_lookups.contains_left(&peer),
            "recovery stalled after a wasted round instead of looking the worker up"
        );
    }

    /// The `outbound_conn_exists` gate: a dial can fail while another connection to the same
    /// worker is up, and chasing a peer we are already connected to is pure waste.
    #[tokio::test(start_paused = true)]
    async fn dial_failure_schedules_nothing_while_a_connection_is_up() {
        let local = Keypair::generate_ed25519();
        let peer = Keypair::generate_ed25519().public().to_peer_id();
        let addr = addr(12208, peer);

        let mut behaviour = stranded_worker(&local, peer, &addr);
        // Drain the reconnect the close queued, so it can't be mistaken for a new one.
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        drain_poll(&mut behaviour);
        assert!(!behaviour.reconnect_scheduled.contains(&peer), "reconnect did not drain");

        let endpoint = dialed(&addr);
        behaviour.on_connection_established(ConnectionEstablished {
            peer_id: peer,
            connection_id: ConnectionId::new_unchecked(3),
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        });

        // A second, redundant dial fails while that connection is still up.
        deliver_dial_failure(&mut behaviour, peer, &transport_error(&addr));
        assert!(
            !behaviour.reconnect_scheduled.contains(&peer),
            "scheduled a reconnect for a worker we are already connected to"
        );
    }

    /// Dropping off the registered set is what stops the retries.
    #[tokio::test(start_paused = true)]
    async fn stops_retrying_once_the_worker_leaves_the_registered_set() {
        let local = Keypair::generate_ed25519();
        let peer = Keypair::generate_ed25519().public().to_peer_id();
        let stale = addr(12208, peer);

        let mut behaviour = stranded_worker(&local, peer, &stale);
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        drain_poll(&mut behaviour);

        behaviour.on_nodes_update(NetworkNodes {
            portals: HashSet::new(),
            workers: HashSet::new(),
        });
        deliver_dial_failure(&mut behaviour, peer, &transport_error(&stale));
        assert!(
            !behaviour.reconnect_scheduled.contains(&peer),
            "kept chasing a worker that is no longer registered"
        );
    }

    /// `NoAddresses` is what a fruitless lookup produces, so scheduling on it would let recovery
    /// feed itself. Out of scope until per-peer backoff exists.
    #[tokio::test(start_paused = true)]
    async fn no_addresses_failure_does_not_schedule_a_lookup() {
        let local = Keypair::generate_ed25519();
        let peer = Keypair::generate_ed25519().public().to_peer_id();
        let stale = addr(12208, peer);

        let mut behaviour = stranded_worker(&local, peer, &stale);
        tokio::time::advance(behaviour.reconnect_cooldown + Duration::from_secs(1)).await;
        drain_poll(&mut behaviour);

        deliver_dial_failure(&mut behaviour, peer, &DialError::NoAddresses);
        assert!(
            !behaviour.reconnect_scheduled.contains(&peer),
            "NoAddresses scheduled a lookup, which lets an empty lookup retry itself"
        );
    }

    /// Only actors in `maintain_worker_connections` mode chase reconnections, and only for peers
    /// the contracts list as registered.
    #[tokio::test(start_paused = true)]
    async fn dial_failure_schedules_nothing_outside_maintained_workers() {
        let local = Keypair::generate_ed25519();
        let peer = Keypair::generate_ed25519().public().to_peer_id();
        let stranger = Keypair::generate_ed25519().public().to_peer_id();
        let stale = addr(12208, peer);

        // Registered, but this actor does not maintain worker connections.
        let mut passive = test_behaviour(&local);
        passive.on_nodes_update(NetworkNodes {
            portals: HashSet::new(),
            workers: HashSet::from([peer]),
        });
        deliver_dial_failure(&mut passive, peer, &transport_error(&stale));
        assert!(
            !passive.reconnect_scheduled.contains(&peer),
            "scheduled a reconnect without maintain_worker_connections"
        );

        // Maintains connections, but the peer is not a registered worker.
        let mut active = stranded_worker(&local, peer, &stale);
        deliver_dial_failure(&mut active, stranger, &transport_error(&addr(12208, stranger)));
        assert!(
            !active.reconnect_scheduled.contains(&stranger),
            "scheduled a reconnect for an unregistered peer"
        );
    }
}
