use std::{
    collections::{HashMap, HashSet},
    iter::zip,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use ethers::types::H160;
use ethers::{
    prelude::{BlockId, Bytes, Middleware, Multicall, Provider},
    types::BlockNumber,
};
use libp2p::futures::Stream;
use num_rational::Ratio;
use tokio_stream::{wrappers::IntervalStream, StreamExt};

use crate::{
    contracts,
    contracts::{
        AllocationsViewer, GatewayRegistry, NetworkController, Strategy, WorkerRegistration,
    },
    transport::Transport,
    Address, ClientError, PeerId, RpcArgs, U256,
};

#[derive(Debug, Clone)]
pub struct Allocation {
    pub worker_peer_id: PeerId,
    pub worker_onchain_id: U256,
    pub computation_units: U256,
}

#[derive(Debug, Clone)]
pub struct GatewayCluster {
    pub operator_addr: Address,
    pub gateway_ids: Vec<PeerId>,
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
    pub gateways: HashSet<PeerId>,
    pub workers: HashSet<PeerId>,
}

impl NetworkNodes {
    pub fn all(self) -> HashSet<PeerId> {
        let mut nodes = self.gateways;
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

    /// Check if gateway (client) is registered on chain
    async fn is_gateway_registered(&self, gateway_id: PeerId) -> Result<bool, ClientError>;

    /// Get worker registration time (None if not registered)
    async fn worker_registration_time(
        &self,
        peer_id: PeerId,
    ) -> Result<Option<SystemTime>, ClientError>;

    /// Get current active gateways
    async fn active_gateways(&self) -> Result<Vec<PeerId>, ClientError>;

    /// Get client's allocations for the current epoch.
    #[deprecated = "Use `gateway_clusters` instead"]
    async fn current_allocations(
        &self,
        gateway_id: PeerId,
        worker_ids: Option<Vec<Worker>>,
    ) -> Result<Vec<Allocation>, ClientError>;

    /// Get the number of compute units available for the portal in the current epoch
    async fn portal_compute_units_per_epoch(&self, portal_id: PeerId) -> Result<u64, ClientError>;

    /// Check if the portal uses the default strategy — allocates CUs evenly among workers
    async fn portal_uses_default_strategy(&self, portal_id: PeerId) -> Result<bool, ClientError>;

    /// Get the current list of all gateway clusters with their allocated CUs for this worker
    async fn gateway_clusters(&self, worker_id: U256) -> Result<Vec<GatewayCluster>, ClientError>;

    /// Get operator address and the total amount of SQD
    /// locked by the operator shared across all portals
    async fn portal_sqd_locked(
        &self,
        portal_id: PeerId,
    ) -> Result<Option<(String, Ratio<u128>)>, ClientError>;

    /// Get a stream of peer IDs of all active network participants (workers & gateways)
    /// Updated on the given interval
    fn network_nodes_stream(self: Box<Self>, interval: Duration) -> NodeStream {
        Box::pin(IntervalStream::new(tokio::time::interval(interval)).then(move |_| {
            let client = self.clone_client();
            async move {
                let (gateways, workers) =
                    tokio::try_join!(client.active_gateways(), client.active_workers())?;
                Ok(NetworkNodes {
                    gateways: gateways.into_iter().collect(),
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

#[derive(Clone)]
struct EthersClient {
    l1_client: Arc<Provider<Transport>>,
    l2_client: Arc<Provider<Transport>>,
    gateway_registry: GatewayRegistry<Provider<Transport>>,
    network_controller: NetworkController<Provider<Transport>>,
    worker_registration: WorkerRegistration<Provider<Transport>>,
    allocations_viewer: AllocationsViewer<Provider<Transport>>,
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
        Ok(Box::new(Self {
            l1_client,
            l2_client,
            gateway_registry,
            worker_registration,
            network_controller,
            allocations_viewer,
            default_strategy_addr,
            multicall_contract_addr: Some(rpc_args.multicall_addr()),
            active_workers_per_page: rpc_args.contract_workers_per_page,
            portals_per_page: U256::from(rpc_args.contract_portals_per_page),
        }))
    }

    async fn multicall(&self) -> Result<Multicall<Provider<Transport>>, ClientError> {
        Ok(contracts::multicall(self.l2_client.clone(), self.multicall_contract_addr).await?)
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

        let latest_block = latest_block_res?.expect("Latest block should be found");
        let hist_block = self
            .l1_client
            .get_block(latest_block.number.unwrap() - epoch_length_blocks as u64)
            .await?
            .expect("Last epoch block should be found");

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
        // A single getActiveWorkers call should be used instead but it lacks pagination and runs out of gas

        let onchain_ids: Vec<U256> =
            self.worker_registration.get_active_worker_ids().call().await?;
        let calls = onchain_ids.chunks(self.active_workers_per_page).map(|ids| async move {
            let mut multicall = self.multicall().await?;
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

    async fn is_gateway_registered(&self, gateway_id: PeerId) -> Result<bool, ClientError> {
        let gateway_id = gateway_id.to_bytes().into();
        let gateway_info: contracts::Gateway =
            self.gateway_registry.get_gateway(gateway_id).call().await?;
        Ok(gateway_info.operator != Address::zero())
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

    async fn active_gateways(&self) -> Result<Vec<PeerId>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;
        let mut active_gateways = Vec::new();
        for page in 0.. {
            let gateway_ids = self
                .gateway_registry
                .get_active_gateways(page.into(), self.portals_per_page)
                .block(latest_block)
                .call()
                .await?;
            let page_size = U256::from(gateway_ids.len());

            active_gateways.extend(gateway_ids.iter().filter_map(|id| PeerId::from_bytes(id).ok()));
            if page_size < self.portals_per_page {
                break;
            }
        }
        Ok(active_gateways)
    }

    async fn current_allocations(
        &self,
        gateway_id: PeerId,
        workers: Option<Vec<Worker>>,
    ) -> Result<Vec<Allocation>, ClientError> {
        let workers = match workers {
            Some(workers) => workers,
            None => self.active_workers().await?,
        };
        if workers.is_empty() {
            return Ok(vec![]);
        }

        let gateway_id: Bytes = gateway_id.to_bytes().into();
        let strategy_addr =
            self.gateway_registry.get_used_strategy(gateway_id.clone()).call().await?;
        let strategy = Strategy::get(strategy_addr, self.l2_client.clone());

        // A little hack to make less requests: default strategy distributes CUs evenly,
        // so we can just query for one worker and return the same number for all.
        if strategy_addr == self.default_strategy_addr {
            let first_worker_id = workers.first().expect("non empty").onchain_id;
            let cus_per_epoch =
                strategy.computation_units_per_epoch(gateway_id, first_worker_id).call().await?;
            return Ok(workers
                .into_iter()
                .map(|w| Allocation {
                    worker_peer_id: w.peer_id,
                    worker_onchain_id: w.onchain_id,
                    computation_units: cus_per_epoch,
                })
                .collect());
        }

        let mut multicall = self.multicall().await?;
        for worker in &workers {
            multicall.add_call::<U256>(
                strategy
                    .method("computationUnitsPerEpoch", (gateway_id.clone(), worker.onchain_id))?,
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
        let id: Bytes = portal_id.to_bytes().into();
        let cus = self.gateway_registry.computation_units_available(id).call().await?;
        Ok(cus.try_into().expect("Computation units should not exceed u64 range"))
    }

    async fn portal_uses_default_strategy(&self, portal_id: PeerId) -> Result<bool, ClientError> {
        let id: Bytes = portal_id.to_bytes().into();
        let strategy_addr = self.gateway_registry.get_used_strategy(id).call().await?;
        Ok(strategy_addr == self.default_strategy_addr)
    }

    async fn gateway_clusters(&self, worker_id: U256) -> Result<Vec<GatewayCluster>, ClientError> {
        let latest_block = self.l2_client.get_block_number().await?;

        let mut clusters = HashMap::new();
        for page in 0.. {
            let allocations = self
                .allocations_viewer
                .get_allocations(worker_id, page.into(), self.portals_per_page)
                .block(latest_block)
                .call()
                .await?;
            let page_size = U256::from(allocations.len());

            for allocation in allocations {
                let Ok(gateway_peer_id) = PeerId::from_bytes(&allocation.gateway_id) else {
                    continue;
                };
                clusters
                    .entry(allocation.operator)
                    .or_insert_with(|| GatewayCluster {
                        operator_addr: allocation.operator,
                        gateway_ids: Vec::new(),
                        allocated_computation_units: allocation.allocated,
                    })
                    .gateway_ids
                    .push(gateway_peer_id);
            }

            if page_size < self.portals_per_page {
                break;
            }
        }
        Ok(clusters.into_values().collect())
    }

    async fn portal_sqd_locked(
        &self,
        portal_id: PeerId,
    ) -> Result<Option<(String, Ratio<u128>)>, ClientError> {
        let id: Bytes = portal_id.to_bytes().into();
        let portal = self.gateway_registry.get_gateway(id).call().await?;

        let operator: H160 = portal.operator;
        if operator.is_zero() {
            return Ok(None);
        }

        let stake = self.gateway_registry.get_stake(operator).call().await?;

        Ok(Some((
            operator.to_string(),
            Ratio::new(stake.amount.as_u128(), 1_000_000_000_000_000_000),
        )))
    }
}
