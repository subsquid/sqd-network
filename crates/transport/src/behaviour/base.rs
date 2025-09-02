use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::NonZeroUsize,
    sync::Arc,
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
        ConnectionClosed, FromSwarm, NetworkBehaviour, ToSwarm,
    },
    StreamProtocol,
};
use libp2p_swarm_derive::NetworkBehaviour;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use sqd_contract_client::{Client as ContractClient, NetworkNodes};

#[cfg(feature = "metrics")]
use crate::metrics::{ACTIVE_CONNECTIONS, ONGOING_QUERIES};
use crate::{
    behaviour::{
        addr_cache::AddressCache,
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

use super::stream_client::{self, ClientBehaviour, ClientConfig, StreamClientHandle};

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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BaseConfig {
    /// How often to check for on-chain updates: current epoch and registered nodes (default: 3 min).
    pub onchain_update_interval: Duration,
    /// Timeout for autoNAT probes (default: 60 sec).
    pub autonat_timeout: Duration,
    /// How often to publish identify info to connected nodes (default: 60 sec).
    pub identify_interval: Duration,
    /// Timeout for kademlia DHT queries (default: 10 sec).
    pub kad_query_timeout: Duration,
    /// Maximum size of gossipsub messages in bytes (default: `MAX_PUBSUB_MSG_SIZE`)
    #[cfg(feature = "pubsub")]
    pub max_pubsub_msg_size: usize,
    /// Maximum number of peers to keep in the address cache (default: 1024)
    pub addr_cache_size: NonZeroUsize,
}

impl BaseConfig {
    pub fn from_env() -> Self {
        let onchain_update_interval =
            Duration::from_secs(parse_env_var("ONCHAIN_UPDATE_INTERVAL_SEC", 60));
        let autonat_timeout = Duration::from_secs(parse_env_var("AUTONAT_TIMEOUT_SEC", 60));
        let identify_interval = Duration::from_secs(parse_env_var("IDENTIFY_INTERVAL_SEC", 60));
        let kad_query_timeout = Duration::from_secs(parse_env_var("KAD_QUERY_TIMEOUT_SEC", 5));
        #[cfg(feature = "pubsub")]
        let max_pubsub_msg_size = parse_env_var("MAX_PUBSUB_MSG_SIZE", MAX_PUBSUB_MSG_SIZE);
        let addr_cache_size = NonZeroUsize::new(parse_env_var("ADDR_CACHE_SIZE", 1024))
            .expect("addr_cache_size should be > 0");
        Self {
            onchain_update_interval,
            autonat_timeout,
            identify_interval,
            kad_query_timeout,
            #[cfg(feature = "pubsub")]
            max_pubsub_msg_size,
            addr_cache_size,
        }
    }
}

pub struct BaseBehaviour {
    inner: InnerBehaviour,
    keypair: Keypair,
    pending_events: VecDeque<TToSwarm<Self>>,
    ongoing_queries: BiHashMap<PeerId, QueryId>,
    outbound_conns: HashMap<PeerId, u32>,
    registered_workers: Arc<RwLock<HashSet<PeerId>>>,
    whitelist_initialized: bool,
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
        kad_config.set_publication_interval(Some(Duration::from_secs(10 * 60)));
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
                contract_client,
                WhitelistConfig::new(config.onchain_update_interval),
            )
            .into(),
            #[cfg(feature = "pubsub")]
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
            ongoing_queries: Default::default(),
            outbound_conns: Default::default(),
            registered_workers: Arc::new(RwLock::new(Default::default())),
            whitelist_initialized: false,
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


    pub fn get_kademlia_mut(&mut self) -> &mut kad::Behaviour<MemoryStore> {
        &mut self.inner.kademlia
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
            InnerBehaviourEvent::Stream(ev) => self.on_stream_event(ev),
            _ => None,
        }
    }

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
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

        None
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

    #[cfg(feature = "pubsub")]
    fn on_pubsub_event(
        &mut self,
        PubsubMsg { peer_id, topic, .. }: PubsubMsg,
    ) -> Option<TToSwarm<Self>> {
        log::trace!("Pub-sub message received: peer_id={peer_id} topic={topic}");
        None
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

        None
    }
}
