use std::{
    collections::{HashMap, HashSet},
    fs,
    iter::zip,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use ethers::{
    prelude::{BlockId, Bytes, Middleware, Multicall, Provider},
    types::{BlockNumber, H160, U64},
};
use libp2p::futures::Stream;
use num_rational::Ratio;
use serde::Deserialize;
use tokio_stream::{wrappers::IntervalStream, StreamExt};

use crate::{
    contracts,
    contracts::{
        AllocationsViewer, GatewayRegistry, NetworkController, PortalRegistry, Strategy,
        WorkerRegistration,
    },
    transport::Transport,
    Address, ClientError, PeerId, RpcArgs, U256,
};

/// Zero value for `bytes32`. `abigen!` emits Solidity `bytes32` as `[u8; 32]`, which has no
/// `is_zero()` helper, so we compare against this constant for the "not registered" branch.
const ZERO_CLUSTER_ID: [u8; 32] = [0u8; 32];

/// SQD token decimals (1e18). Used to scale `Stake.amount` / `Cluster.totalStaked` (in wei) to a
/// human-readable `Ratio<u128>`.
const SQD_DECIMALS_DENOM: u128 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone)]
pub struct Allocation {
    pub worker_peer_id: PeerId,
    pub worker_onchain_id: U256,
    pub computation_units: U256,
}

#[derive(Debug, Clone)]
pub struct PortalCluster {
    pub operator_addr: Address,
    pub portal_ids: Vec<PeerId>,
    pub allocated_computation_units: U256,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Worker {
    pub peer_id: PeerId,
    pub onchain_id: U256,
    pub address: Address,
    pub bond: U256,
    pub registered_at: u128,
    pub deregistered_at: Option<u128>,
}

impl Worker {
    fn new(worker: &contracts::Worker, onchain_id: U256) -> Result<Self, ClientError> {
        let peer_id = PeerId::from_bytes(&worker.peer_id)?;
        let deregistered_at = (worker.deregistered_at > 0).then_some(worker.deregistered_at);
        Ok(Self {
            peer_id,
            onchain_id,
            address: worker.creator,
            bond: worker.bond,
            registered_at: worker.registered_at,
            deregistered_at,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NetworkNodes {
    pub portals: HashSet<PeerId>,
    pub workers: HashSet<PeerId>,
}

impl NetworkNodes {
    pub fn all(self) -> HashSet<PeerId> {
        let mut nodes = self.portals;
        nodes.extend(self.workers);
        nodes
    }
}

pub type NodeStream =
    Pin<Box<dyn Stream<Item = Result<NetworkNodes, ClientError>> + Send + 'static>>;

/// A stream of (epoch_number, epoch_start_time)
pub type EpochStream =
    Pin<Box<dyn Stream<Item = Result<(u32, SystemTime), ClientError>> + Send + 'static>>;

#[async_trait]
pub trait Client: Send + Sync + 'static {
    /// Using regular clone is not possible for trait objects
    fn clone_client(&self) -> Box<dyn Client>;

    /// Get the current epoch number
    async fn current_epoch(&self) -> Result<u32, ClientError>;

    /// Get the time when the current epoch started
    async fn current_epoch_start(&self) -> Result<SystemTime, ClientError>;

    /// Get the approximate length of the current epoch
    async fn epoch_length(&self) -> Result<Duration, ClientError>;

    /// Get the on-chain ID for the worker
    async fn worker_id(&self, peer_id: PeerId) -> Result<U256, ClientError>;

    /// Get current active worker set
    async fn active_workers(&self) -> Result<Vec<Worker>, ClientError>;

    /// Check if portal (client) is registered on chain
    async fn is_portal_registered(&self, portal_id: PeerId) -> Result<bool, ClientError>;

    /// Get worker registration time (None if not registered)
    async fn worker_registration_time(
        &self,
        peer_id: PeerId,
    ) -> Result<Option<SystemTime>, ClientError>;

    /// Get current active portals
    async fn active_portals(&self) -> Result<Vec<PeerId>, ClientError>;

    /// Get client's allocations for the current epoch.
    #[deprecated = "Use `portal_clusters` instead"]
    async fn current_allocations(
        &self,
        portal_id: PeerId,
        worker_ids: Option<Vec<Worker>>,
    ) -> Result<Vec<Allocation>, ClientError>;

    /// Get the number of compute units available for the portal in the current epoch
    async fn portal_compute_units_per_epoch(&self, portal_id: PeerId) -> Result<u64, ClientError>;

    /// Check if the portal uses the default strategy — allocates CUs evenly among workers
    async fn portal_uses_default_strategy(&self, portal_id: PeerId) -> Result<bool, ClientError>;

    /// Get the current list of all portal clusters with their allocated CUs for this worker
    async fn portal_clusters(&self, worker_id: U256) -> Result<Vec<PortalCluster>, ClientError>;

    /// Get operator address and the total amount of SQD
    /// locked by the operator shared across all portals
    async fn portal_sqd_locked(
        &self,
        portal_id: PeerId,
    ) -> Result<Option<(String, Ratio<u128>)>, ClientError>;

    /// Get a stream of peer IDs of all active network participants (workers & portals)
    /// Updated on the given interval
    fn network_nodes_stream(self: Box<Self>, interval: Duration) -> NodeStream {
        Box::pin(IntervalStream::new(tokio::time::interval(interval)).then(move |_| {
            let client = self.clone_client();
            async move {
                let (portals, workers) =
                    tokio::try_join!(client.active_portals(), client.active_workers())?;
                Ok(NetworkNodes {
                    portals: portals.into_iter().collect(),
                    workers: workers.into_iter().map(|w| w.peer_id).collect(),
                })
            }
        }))
    }

    /// Get a stream of current epoch number and start time
    /// Updated on the given interval
    #[deprecated = "This function makes 3 RPC calls, use `current_epoch` and `current_epoch_start` separately"]
    fn epoch_stream(self: Box<Self>, interval: Duration) -> EpochStream {
        Box::pin(IntervalStream::new(tokio::time::interval(interval)).then(move |_| {
            let client = self.clone_client();
            async move {
                let epoch_number = client.current_epoch().await?;
                let epoch_start = client.current_epoch_start().await?;
                Ok((epoch_number, epoch_start))
            }
        }))
    }
}

pub async fn get_client(rpc_args: &RpcArgs) -> Result<Box<dyn Client>, ClientError> {
    // Check if dummy client file path is set
    if let Some(dummy_file_path) = &rpc_args.dummy_client_file_path {
        log::info!("Using dummy client from file: {}", dummy_file_path);
        let data = fs::read(dummy_file_path)?;
        let dummy_data: DummyData = serde_json::from_slice(&data)?;
        let client: Box<dyn Client> = Box::new(DummyClient::new(dummy_data));
        return Ok(client);
    }

    log::info!(
        "Initializing contract client. network={:?} rpc_url={} l1_rpc_url={}",
        rpc_args.network,
        rpc_args.rpc_url,
        rpc_args.l1_rpc_url
    );
    let l2_client = Transport::connect(&rpc_args.rpc_url).await?;
    let l1_client = Transport::connect(&rpc_args.l1_rpc_url).await?;
    let client: Box<dyn Client> = EthersClient::new(l1_client, l2_client, rpc_args).await?;
    Ok(client)
}

/// Dummy data structure loaded from JSON file
#[derive(Debug, Clone, Deserialize)]
pub struct DummyData {
    pub current_epoch: u32,
    pub current_epoch_start: u64,
    pub epoch_length_secs: u64,
    pub workers: Vec<DummyWorker>,
    pub portals: Vec<String>,
    pub portal_clusters: Vec<DummyPortalCluster>,
    pub worker_ids: HashMap<String, String>,
    pub portal_compute_units: HashMap<String, u64>,
    pub portal_uses_default_strategy: HashMap<String, bool>,
    pub portal_sqd_locked: HashMap<String, Option<(String, f64)>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DummyWorker {
    pub peer_id: String,
    pub onchain_id: String,
    pub address: String,
    pub bond: String,
    pub registered_at: u128,
    pub deregistered_at: Option<u128>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DummyPortalCluster {
    pub operator_addr: String,
    pub portal_ids: Vec<String>,
    pub allocated_computation_units: String,
}

#[derive(Clone)]
struct DummyClient {
    data: DummyData,
}

impl DummyClient {
    pub fn new(data: DummyData) -> Self {
        Self { data }
    }

    fn parse_peer_id(peer_id_str: &str) -> Result<PeerId, ClientError> {
        peer_id_str.parse().map_err(ClientError::from)
    }

    fn parse_u256(u256_str: &str) -> Result<U256, ClientError> {
        u256_str
            .parse()
            .map_err(|_| ClientError::Contract(format!("Invalid U256: {}", u256_str)))
    }

    fn parse_address(addr_str: &str) -> Result<Address, ClientError> {
        addr_str
            .parse()
            .map_err(|_| ClientError::Contract(format!("Invalid Address: {}", addr_str)))
    }
}

#[async_trait]
impl Client for DummyClient {
    fn clone_client(&self) -> Box<dyn Client> {
        Box::new(self.clone())
    }

    async fn current_epoch(&self) -> Result<u32, ClientError> {
        Ok(self.data.current_epoch)
    }

    async fn current_epoch_start(&self) -> Result<SystemTime, ClientError> {
        Ok(UNIX_EPOCH + Duration::from_secs(self.data.current_epoch_start))
    }

    async fn epoch_length(&self) -> Result<Duration, ClientError> {
        Ok(Duration::from_secs(self.data.epoch_length_secs))
    }

    async fn worker_id(&self, peer_id: PeerId) -> Result<U256, ClientError> {
        let peer_id_str = peer_id.to_string();
        if let Some(id_str) = self.data.worker_ids.get(&peer_id_str) {
            Self::parse_u256(id_str)
        } else {
            Ok(U256::zero())
        }
    }

    async fn active_workers(&self) -> Result<Vec<Worker>, ClientError> {
        self.data
            .workers
            .iter()
            .map(|w| {
                Ok(Worker {
                    peer_id: Self::parse_peer_id(&w.peer_id)?,
                    onchain_id: Self::parse_u256(&w.onchain_id)?,
                    address: Self::parse_address(&w.address)?,
                    bond: Self::parse_u256(&w.bond)?,
                    registered_at: w.registered_at,
                    deregistered_at: w.deregistered_at,
                })
            })
            .collect()
    }

    async fn is_portal_registered(&self, portal_id: PeerId) -> Result<bool, ClientError> {
        Ok(self.data.portals.contains(&portal_id.to_string()))
    }

    async fn worker_registration_time(
        &self,
        peer_id: PeerId,
    ) -> Result<Option<SystemTime>, ClientError> {
        let peer_id_str = peer_id.to_string();
        if let Some(worker) = self.data.workers.iter().find(|w| w.peer_id == peer_id_str) {
            if worker.registered_at > 0 {
                return Ok(Some(UNIX_EPOCH + Duration::from_secs(worker.registered_at as u64)));
            }
        }
        Ok(None)
    }

    async fn active_portals(&self) -> Result<Vec<PeerId>, ClientError> {
        self.data.portals.iter().map(|s| Self::parse_peer_id(s)).collect()
    }

    async fn current_allocations(
        &self,
        portal_id: PeerId,
        workers: Option<Vec<Worker>>,
    ) -> Result<Vec<Allocation>, ClientError> {
        let workers = match workers {
            Some(workers) => workers,
            None => self.active_workers().await?,
        };
        if workers.is_empty() {
            return Ok(vec![]);
        }
        let portal_id_str = portal_id.to_string();
        let cus_per_epoch =
            self.data.portal_compute_units.get(&portal_id_str).copied().unwrap_or(0);
        Ok(workers
            .into_iter()
            .map(|w| Allocation {
                worker_peer_id: w.peer_id,
                worker_onchain_id: w.onchain_id,
                computation_units: U256::from(cus_per_epoch),
            })
            .collect())
    }

    async fn portal_compute_units_per_epoch(&self, portal_id: PeerId) -> Result<u64, ClientError> {
        let portal_id_str = portal_id.to_string();
        Ok(self.data.portal_compute_units.get(&portal_id_str).copied().unwrap_or(0))
    }

    async fn portal_uses_default_strategy(&self, portal_id: PeerId) -> Result<bool, ClientError> {
        let portal_id_str = portal_id.to_string();
        Ok(self
            .data
            .portal_uses_default_strategy
            .get(&portal_id_str)
            .copied()
            .unwrap_or(true))
    }

    async fn portal_clusters(&self, _worker_id: U256) -> Result<Vec<PortalCluster>, ClientError> {
        self.data
            .portal_clusters
            .iter()
            .map(|c| {
                Ok(PortalCluster {
                    operator_addr: Self::parse_address(&c.operator_addr)?,
                    portal_ids: c
                        .portal_ids
                        .iter()
                        .map(|s| Self::parse_peer_id(s))
                        .collect::<Result<Vec<_>, _>>()?,
                    allocated_computation_units: Self::parse_u256(&c.allocated_computation_units)?,
                })
            })
            .collect()
    }

    async fn portal_sqd_locked(
        &self,
        portal_id: PeerId,
    ) -> Result<Option<(String, Ratio<u128>)>, ClientError> {
        let portal_id_str = portal_id.to_string();
        if let Some(data) = self.data.portal_sqd_locked.get(&portal_id_str) {
            match data {
                Some((addr, ratio)) => Ok(Some((addr.clone(), Ratio::new_raw(*ratio as u128, 1)))),
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone)]
struct EthersClient {
    l1_client: Arc<Provider<Transport>>,
    l2_client: Arc<Provider<Transport>>,
    gateway_registry: GatewayRegistry<Provider<Transport>>,
    network_controller: NetworkController<Provider<Transport>>,
    worker_registration: WorkerRegistration<Provider<Transport>>,
    allocations_viewer: AllocationsViewer<Provider<Transport>>,
    /// New PortalRegistry handle. `None` when the registry is disabled via the rollback flag
    /// or when no address is configured for the active network (e.g. Tethys without an env
    /// override). Every method that consults the new registry must guard on this `Option`.
    portal_registry: Option<PortalRegistry<Provider<Transport>>>,
    default_strategy_addr: Address,
    multicall_contract_addr: Option<Address>,
    active_workers_per_page: usize,
    portals_per_page: U256,
}

impl EthersClient {
    pub async fn new(
        l1_client: Arc<Provider<Transport>>,
        l2_client: Arc<Provider<Transport>>,
        rpc_args: &RpcArgs,
    ) -> Result<Box<Self>, ClientError> {
        log::info!("GatewayRegistry: {}", rpc_args.gateway_registry_addr());
        let gateway_registry =
            GatewayRegistry::get(l2_client.clone(), rpc_args.gateway_registry_addr());
        let default_strategy_addr = gateway_registry.default_strategy().call().await?;
        log::info!("Default strategy: {default_strategy_addr}");
        log::info!("NetworkController: {}", rpc_args.network_controller_addr());
        let network_controller =
            NetworkController::get(l2_client.clone(), rpc_args.network_controller_addr());
        log::info!("WorkerRegistration: {}", rpc_args.worker_registration_addr());
        let worker_registration =
            WorkerRegistration::get(l2_client.clone(), rpc_args.worker_registration_addr());
        log::info!("AllocationsViewer: {}", rpc_args.allocations_viewer_addr());
        let allocations_viewer =
            AllocationsViewer::get(l2_client.clone(), rpc_args.allocations_viewer_addr());
        log::info!("Multicall: {}", rpc_args.multicall_addr());

        // Portal Pools migration: try the new PortalRegistry first, fall back to the legacy
        // GatewayRegistry/AllocationsViewer when the peer ID isn't migrated yet. The registry
        // is opt-out (via --disable-portal-registry) on Mainnet and opt-in (no default address)
        // on Tethys, so a missing address simply degrades to legacy-only behavior.
        let portal_registry =
            match (rpc_args.disable_portal_registry, rpc_args.portal_registry_addr()) {
                (true, _) => {
                    log::info!("PortalRegistry: disabled by --disable-portal-registry");
                    None
                }
                (false, Some(addr)) => {
                    log::info!("PortalRegistry: {addr}");
                    Some(PortalRegistry::get(l2_client.clone(), addr))
                }
                (false, None) => {
                    log::info!(
                        "PortalRegistry: no address configured for {} — set \
                         PORTAL_REGISTRY_CONTRACT_ADDR to enable",
                        rpc_args.network
                    );
                    None
                }
            };

        Ok(Box::new(Self {
            l1_client,
            l2_client,
            gateway_registry,
            worker_registration,
            network_controller,
            allocations_viewer,
            portal_registry,
            default_strategy_addr,
            multicall_contract_addr: Some(rpc_args.multicall_addr()),
            active_workers_per_page: rpc_args.contract_workers_per_page,
            portals_per_page: U256::from(rpc_args.contract_portals_per_page),
        }))
    }

    async fn multicall(&self) -> Result<Multicall<Provider<Transport>>, ClientError> {
        Ok(contracts::multicall(self.l2_client.clone(), self.multicall_contract_addr).await?)
    }

    async fn active_workers_at(&self, latest_block: U64) -> Result<Vec<Worker>, ClientError> {
        // A single getActiveWorkers call should be used instead but it lacks pagination and runs out of gas

        let onchain_ids: Vec<U256> = self
            .worker_registration
            .get_active_worker_ids()
            .block(latest_block)
            .call()
            .await?;
        let calls = onchain_ids.chunks(self.active_workers_per_page).map(|ids| async move {
            let mut multicall = self.multicall().await?.block(latest_block);
            for id in ids {
                multicall.add_call::<contracts::Worker>(
                    self.worker_registration.method("getWorker", *id)?,
                    false,
                );
            }
            let workers: Vec<contracts::Worker> = multicall.call_array().await?;
            Result::<_, ClientError>::Ok(workers)
        });

        let workers = futures::future::try_join_all(calls).await?.into_iter().flatten();

        let workers = workers
            .zip(onchain_ids)
            .filter_map(|(worker, onchain_id)| match Worker::new(&worker, onchain_id) {
                Ok(worker) => Some(worker),
                Err(e) => {
                    log::debug!("Error reading worker from chain: {e:?}");
                    None
                }
            })
            .collect();
        Ok(workers)
    }
}

#[async_trait]
impl Client for EthersClient {
    fn clone_client(&self) -> Box<dyn Client> {
        Box::new(self.clone())
    }

    async fn current_epoch(&self) -> Result<u32, ClientError> {
        let epoch = self
            .network_controller
            .epoch_number()
            .call()
            .await?
            .try_into()
            .expect("Epoch number should not exceed u32 range");
        Ok(epoch)
    }

    async fn current_epoch_start(&self) -> Result<SystemTime, ClientError> {
        // TODO: make it a single multicall
        let next_epoch_call = self.network_controller.next_epoch();
        let epoch_length_call = self.network_controller.worker_epoch_length();
        let (next_epoch_start_block, epoch_length_blocks) =
            tokio::try_join!(next_epoch_call.call(), epoch_length_call.call())?;
        let block_num: u64 = (next_epoch_start_block - epoch_length_blocks)
            .try_into()
            .expect("Epoch number should not exceed u64 range");
        log::debug!("Current epoch: {block_num} Epoch length: {epoch_length_blocks} Next epoch: {next_epoch_start_block}");
        // Blocks returned by `next_epoch()` and `epoch_length()` are **L1 blocks**
        let block = self
            .l1_client
            .get_block(BlockId::Number(block_num.into()))
            .await?
            .ok_or(ClientError::BlockNotFound)?;
        Ok(UNIX_EPOCH + Duration::from_secs(block.timestamp.as_u64()))
    }

    async fn epoch_length(&self) -> Result<Duration, ClientError> {
        let epoch_length_call = self.network_controller.worker_epoch_length();
        let latest_block_call = self.l1_client.get_block(BlockNumber::Latest);
        let (epoch_length_blocks_res, latest_block_res) =
            tokio::join!(epoch_length_call.call(), latest_block_call);
        let epoch_length_blocks = epoch_length_blocks_res?;

        let latest_block = latest_block_res?.ok_or(ClientError::BlockNotFound)?;
        let hist_block = self
            .l1_client
            .get_block(latest_block.number.unwrap() - epoch_length_blocks as u64)
            .await?
            .ok_or(ClientError::BlockNotFound)?;

        Ok(Duration::from_secs(
            (latest_block.timestamp - hist_block.timestamp)
                .try_into()
                .expect("Epoch length should not exceed u64 range"),
        ))
    }

    async fn worker_id(&self, peer_id: PeerId) -> Result<U256, ClientError> {
        let peer_id = peer_id.to_bytes().into();
        let id: U256 = self.worker_registration.worker_ids(peer_id).call().await?;
        Ok(id)
    }

    async fn active_workers(&self) -> Result<Vec<Worker>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;
        self.active_workers_at(latest_block).await
    }

    async fn is_portal_registered(&self, portal_id: PeerId) -> Result<bool, ClientError> {
        let id_bytes: Bytes = portal_id.to_bytes().into();
        let latest_block = self.l2_client.get_block_number().await?;
        if let Some(pr) = &self.portal_registry {
            let cluster_id = pr
                .get_cluster_id_by_peer_id(id_bytes.clone())
                .block(latest_block)
                .call()
                .await?;
            if cluster_id != ZERO_CLUSTER_ID {
                log::debug!("portal {portal_id} resolved via PortalRegistry");
                return Ok(true);
            }
        }
        let portal_info: contracts::Gateway =
            self.gateway_registry.get_gateway(id_bytes).block(latest_block).call().await?;
        let registered = portal_info.operator != Address::zero();
        if registered {
            log::debug!("portal {portal_id} resolved via GatewayRegistry");
        }
        Ok(registered)
    }

    async fn worker_registration_time(
        &self,
        peer_id: PeerId,
    ) -> Result<Option<SystemTime>, ClientError> {
        let worker_id =
            self.worker_registration.worker_ids(peer_id.to_bytes().into()).call().await?;
        if worker_id.is_zero() {
            log::info!("Worker {peer_id} not registered on chain");
            return Ok(None);
        }
        let worker = self.worker_registration.get_worker(worker_id).call().await?;
        let block_num: u64 = worker
            .registered_at
            .try_into()
            .expect("Block number should not exceed u64 range");
        let Some(block) = self.l1_client.get_block(BlockId::Number(block_num.into())).await? else {
            log::info!("Worker {peer_id} on-chain registration pending");
            return Ok(None); // If block is not found, it means the worker has not been registered yet
        };
        Ok(Some(UNIX_EPOCH + Duration::from_secs(block.timestamp.as_u64())))
    }

    async fn active_portals(&self) -> Result<Vec<PeerId>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;

        // PortalRegistry: enumerate active clusters and flatten their portal lists.
        let mut new_portals: Vec<PeerId> = Vec::new();
        if let Some(pr) = &self.portal_registry {
            let mut offset = U256::zero();
            let limit = self.portals_per_page;
            loop {
                let (_cluster_ids, clusters, total_active) =
                    pr.get_active_clusters(offset, limit).block(latest_block).call().await?;
                let page_len = clusters.len();
                for c in &clusters {
                    for p in &c.portals {
                        if let Ok(peer_id) = PeerId::from_bytes(&p.peer_id) {
                            new_portals.push(peer_id);
                        }
                    }
                }
                offset += U256::from(page_len);
                if page_len == 0 || offset >= total_active {
                    break;
                }
            }
        }

        // Legacy GatewayRegistry: keep the old enumeration so un-migrated gateways stay served.
        let mut legacy_portals: Vec<PeerId> = Vec::new();
        for page in 0.. {
            let portal_ids = self
                .gateway_registry
                .get_active_gateways(page.into(), self.portals_per_page)
                .block(latest_block)
                .call()
                .await?;
            let page_size = U256::from(portal_ids.len());
            legacy_portals.extend(portal_ids.iter().filter_map(|id| PeerId::from_bytes(id).ok()));
            if page_size < self.portals_per_page {
                break;
            }
        }

        Ok(crate::merge::merge_active_portals(new_portals, legacy_portals))
    }

    async fn current_allocations(
        &self,
        portal_id: PeerId,
        workers: Option<Vec<Worker>>,
    ) -> Result<Vec<Allocation>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;
        let workers = match workers {
            Some(workers) => workers,
            None => self.active_workers_at(latest_block).await?,
        };
        if workers.is_empty() {
            return Ok(vec![]);
        }

        let id_bytes: Bytes = portal_id.to_bytes().into();

        // PortalRegistry path: equal-split the cluster's CUs across all active workers, then
        // project that per-worker value onto the caller's requested worker set.
        if let Some(pr) = &self.portal_registry {
            let cluster_id = pr
                .get_cluster_id_by_peer_id(id_bytes.clone())
                .block(latest_block)
                .call()
                .await?;
            if cluster_id != ZERO_CLUSTER_ID {
                log::debug!("portal {portal_id} resolved via PortalRegistry");
                let cus_total =
                    pr.get_computation_units(cluster_id).block(latest_block).call().await?;
                let active_worker_count: U256 = self
                    .worker_registration
                    .get_active_worker_count()
                    .block(latest_block)
                    .call()
                    .await?;
                if active_worker_count.is_zero() {
                    return Ok(vec![]);
                }
                let per_worker = cus_total / active_worker_count;
                return Ok(workers
                    .into_iter()
                    .map(|w| Allocation {
                        worker_peer_id: w.peer_id,
                        worker_onchain_id: w.onchain_id,
                        computation_units: per_worker,
                    })
                    .collect());
            }
        }

        // Legacy GatewayRegistry strategy path.
        log::debug!("portal {portal_id} resolved via GatewayRegistry");
        let strategy_addr = self
            .gateway_registry
            .get_used_strategy(id_bytes.clone())
            .block(latest_block)
            .call()
            .await?;
        let strategy = Strategy::get(strategy_addr, self.l2_client.clone());

        // A little hack to make less requests: default strategy distributes CUs evenly,
        // so we can just query for one worker and return the same number for all.
        if strategy_addr == self.default_strategy_addr {
            let first_worker_id = workers.first().expect("non empty").onchain_id;
            let cus_per_epoch = strategy
                .computation_units_per_epoch(id_bytes, first_worker_id)
                .block(latest_block)
                .call()
                .await?;
            return Ok(workers
                .into_iter()
                .map(|w| Allocation {
                    worker_peer_id: w.peer_id,
                    worker_onchain_id: w.onchain_id,
                    computation_units: cus_per_epoch,
                })
                .collect());
        }

        let mut multicall = self.multicall().await?.block(latest_block);
        for worker in &workers {
            multicall.add_call::<U256>(
                strategy
                    .method("computationUnitsPerEpoch", (id_bytes.clone(), worker.onchain_id))?,
                false,
            );
        }
        let compute_units: Vec<U256> = multicall.call_array().await?;
        Ok(zip(workers, compute_units)
            .map(|(w, cus)| Allocation {
                worker_peer_id: w.peer_id,
                worker_onchain_id: w.onchain_id,
                computation_units: cus,
            })
            .collect())
    }

    async fn portal_compute_units_per_epoch(&self, portal_id: PeerId) -> Result<u64, ClientError> {
        let id_bytes: Bytes = portal_id.to_bytes().into();
        let latest_block = self.l2_client.get_block_number().await?;
        if let Some(pr) = &self.portal_registry {
            let cluster_id = pr
                .get_cluster_id_by_peer_id(id_bytes.clone())
                .block(latest_block)
                .call()
                .await?;
            if cluster_id != ZERO_CLUSTER_ID {
                log::debug!("portal {portal_id} resolved via PortalRegistry");
                let cus = pr.get_computation_units(cluster_id).block(latest_block).call().await?;
                return Ok(cus.try_into().expect("Computation units should not exceed u64 range"));
            }
        }
        log::debug!("portal {portal_id} resolved via GatewayRegistry");
        let cus = self
            .gateway_registry
            .computation_units_available(id_bytes)
            .block(latest_block)
            .call()
            .await?;
        Ok(cus.try_into().expect("Computation units should not exceed u64 range"))
    }

    async fn portal_uses_default_strategy(&self, portal_id: PeerId) -> Result<bool, ClientError> {
        let id_bytes: Bytes = portal_id.to_bytes().into();
        let latest_block = self.l2_client.get_block_number().await?;
        // PortalRegistry has no strategy concept — every cluster behaves like an equal-split
        // default strategy. If the peer is registered there, return true unconditionally.
        if let Some(pr) = &self.portal_registry {
            let cluster_id = pr
                .get_cluster_id_by_peer_id(id_bytes.clone())
                .block(latest_block)
                .call()
                .await?;
            if cluster_id != ZERO_CLUSTER_ID {
                log::debug!("portal {portal_id} resolved via PortalRegistry");
                return Ok(true);
            }
        }
        log::debug!("portal {portal_id} resolved via GatewayRegistry");
        let strategy_addr = self
            .gateway_registry
            .get_used_strategy(id_bytes)
            .block(latest_block)
            .call()
            .await?;
        Ok(strategy_addr == self.default_strategy_addr)
    }

    async fn portal_clusters(&self, worker_id: U256) -> Result<Vec<PortalCluster>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;

        // PortalRegistry: equal-split each active cluster's CUs across the active worker set.
        let mut new_clusters: Vec<PortalCluster> = Vec::new();
        if let Some(pr) = &self.portal_registry {
            let active_worker_count: U256 = self
                .worker_registration
                .get_active_worker_count()
                .block(latest_block)
                .call()
                .await?;
            if !active_worker_count.is_zero() {
                let mut offset = U256::zero();
                let limit = self.portals_per_page;
                loop {
                    let (cluster_ids, clusters, total_active) =
                        pr.get_active_clusters(offset, limit).block(latest_block).call().await?;
                    let page_len = clusters.len();

                    let cluster_cus: Vec<U256> = if cluster_ids.is_empty() {
                        Vec::new()
                    } else {
                        let mut mc = self.multicall().await?.block(latest_block);
                        for cid in &cluster_ids {
                            mc.add_call::<U256>(pr.method("getComputationUnits", *cid)?, false);
                        }
                        mc.call_array().await?
                    };

                    for (cluster, cus_total) in clusters.iter().zip(cluster_cus) {
                        let per_worker = cus_total / active_worker_count;
                        if per_worker.is_zero() {
                            continue;
                        }
                        let portal_ids: Vec<PeerId> = cluster
                            .portals
                            .iter()
                            .filter_map(|p| PeerId::from_bytes(&p.peer_id).ok())
                            .collect();
                        if portal_ids.is_empty() {
                            continue;
                        }
                        new_clusters.push(PortalCluster {
                            operator_addr: cluster.operator,
                            portal_ids,
                            allocated_computation_units: per_worker,
                        });
                    }

                    offset += U256::from(page_len);
                    if page_len == 0 || offset >= total_active {
                        break;
                    }
                }
            }
        }

        // Legacy AllocationsViewer rows; the merge layer dedupes migrated peer IDs and groups
        // surviving entries by operator.
        let mut legacy_allocations: Vec<contracts::Allocation> = Vec::new();
        for page in 0.. {
            let allocations = self
                .allocations_viewer
                .get_allocations(worker_id, page.into(), self.portals_per_page)
                .block(latest_block)
                .call()
                .await?;
            let page_size = U256::from(allocations.len());
            legacy_allocations.extend(allocations);
            if page_size < self.portals_per_page {
                break;
            }
        }

        Ok(crate::merge::merge_portal_clusters(new_clusters, legacy_allocations))
    }

    async fn portal_sqd_locked(
        &self,
        portal_id: PeerId,
    ) -> Result<Option<(String, Ratio<u128>)>, ClientError> {
        let id_bytes: Bytes = portal_id.to_bytes().into();
        let latest_block = self.l2_client.get_block_number().await?;

        if let Some(pr) = &self.portal_registry {
            let cluster_id = pr
                .get_cluster_id_by_peer_id(id_bytes.clone())
                .block(latest_block)
                .call()
                .await?;
            if cluster_id != ZERO_CLUSTER_ID {
                log::debug!("portal {portal_id} resolved via PortalRegistry");
                let cluster = pr.get_cluster(cluster_id).block(latest_block).call().await?;
                if !cluster.operator.is_zero() {
                    return Ok(Some((
                        cluster.operator.to_string(),
                        Ratio::new(cluster.total_staked.as_u128(), SQD_DECIMALS_DENOM),
                    )));
                }
            }
        }

        log::debug!("portal {portal_id} resolved via GatewayRegistry");
        let portal = self.gateway_registry.get_gateway(id_bytes).block(latest_block).call().await?;
        let operator: H160 = portal.operator;
        if operator.is_zero() {
            return Ok(None);
        }
        let stake = self.gateway_registry.get_stake(operator).block(latest_block).call().await?;
        Ok(Some((
            operator.to_string(),
            Ratio::new(stake.amount.as_u128(), SQD_DECIMALS_DENOM),
        )))
    }
}
