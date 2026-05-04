use clap::{Args, ValueEnum};

use crate::Address;

#[derive(Args, Clone)]
pub struct RpcArgs {
    /// Blockchain RPC URL
    #[arg(long, env)]
    pub rpc_url: String,

    /// Layer 1 blockchain RPC URL
    #[arg(long, env)]
    pub l1_rpc_url: String,

    #[command(flatten)]
    contract_addrs: ContractAddrs,

    /// Network to connect to (mainnet or testnet)
    #[arg(long, env, default_value = "mainnet")]
    pub network: Network,

    #[arg(long, env, hide(true), default_value_t = 500)]
    pub contract_workers_per_page: usize,

    #[arg(long, env, hide(true), default_value_t = 5)]
    pub contract_portals_per_page: usize,

    /// Path to dummy client data file (if set, use dummy client instead of EthersClient)
    #[arg(long, env)]
    pub dummy_client_file_path: Option<String>,

    /// Disable the new PortalRegistry path entirely (fall back to legacy GatewayRegistry only).
    /// Useful as a rollback switch during the PortalRegistry migration.
    #[arg(long, env, default_value_t = false)]
    pub disable_portal_registry: bool,
}

impl RpcArgs {
    pub fn gateway_registry_addr(&self) -> Address {
        self.contract_addrs
            .gateway_registry_contract_addr
            .unwrap_or_else(|| self.network.gateway_registry_default_addr())
    }

    pub fn worker_registration_addr(&self) -> Address {
        self.contract_addrs
            .worker_registration_contract_addr
            .unwrap_or_else(|| self.network.worker_registration_default_addr())
    }

    pub fn network_controller_addr(&self) -> Address {
        self.contract_addrs
            .network_controller_contract_addr
            .unwrap_or_else(|| self.network.network_controller_default_addr())
    }

    pub fn allocations_viewer_addr(&self) -> Address {
        self.contract_addrs
            .allocations_viewer_contract_addr
            .unwrap_or_else(|| self.network.allocations_viewer_default_addr())
    }

    pub fn multicall_addr(&self) -> Address {
        self.contract_addrs
            .multicall_contract_addr
            .unwrap_or_else(|| self.network.multicall_default_addr())
    }

    /// Returns the configured PortalRegistry address, falling back to the network default.
    /// May be `None` (e.g. on Tethys, which is opt-in only) — callers must handle that case
    /// to keep the legacy-only code path operational.
    pub fn portal_registry_addr(&self) -> Option<Address> {
        self.contract_addrs
            .portal_registry_contract_addr
            .or_else(|| self.network.portal_registry_default_addr())
    }
}

#[derive(Args, Clone)]
pub struct ContractAddrs {
    #[arg(long, env)]
    pub gateway_registry_contract_addr: Option<Address>,
    #[arg(long, env)]
    pub worker_registration_contract_addr: Option<Address>,
    #[arg(long, env)]
    pub network_controller_contract_addr: Option<Address>,
    #[arg(long, env)]
    pub allocations_viewer_contract_addr: Option<Address>,
    #[arg(long, env)]
    pub multicall_contract_addr: Option<Address>,
    /// New PortalRegistry contract address. On mainnet this defaults to the deployed proxy;
    /// on Tethys it defaults to `None` and operators must opt in via this env var.
    #[arg(long, env)]
    pub portal_registry_contract_addr: Option<Address>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab_case")]
pub enum Network {
    Tethys,
    Mainnet,
}

impl Network {
    pub fn gateway_registry_default_addr(&self) -> Address {
        match self {
            Self::Tethys => "0xAB46F688AbA4FcD1920F21E9BD16B229316D8b0a".parse().unwrap(),
            Self::Mainnet => "0x8A90A1cE5fa8Cf71De9e6f76B7d3c0B72feB8c4b".parse().unwrap(),
        }
    }

    pub fn worker_registration_default_addr(&self) -> Address {
        match self {
            Self::Tethys => "0xCD8e983F8c4202B0085825Cf21833927D1e2b6Dc".parse().unwrap(),
            Self::Mainnet => "0x36E2B147Db67E76aB67a4d07C293670EbeFcAE4E".parse().unwrap(),
        }
    }

    pub fn network_controller_default_addr(&self) -> Address {
        match self {
            Self::Tethys => "0x018a4EC4B1f5D03F93d34Fd7F0bAfc69B66B97A1".parse().unwrap(),
            Self::Mainnet => "0x159550d2589CfF1Ff604AF715130642256B88847".parse().unwrap(),
        }
    }

    pub fn allocations_viewer_default_addr(&self) -> Address {
        match self {
            Self::Tethys => "0xC0Af6432947db51e0C179050dAF801F19d40D2B7".parse().unwrap(),
            Self::Mainnet => "0x88CE6D8D70df9Fe049315fd9D6c3d59108C15c4C".parse().unwrap(),
        }
    }

    pub fn multicall_default_addr(&self) -> Address {
        // The match is here so that adding new network forces programmer to check the multicall address
        match self {
            Self::Tethys | Self::Mainnet => {
                "0xcA11bde05977b3631167028862bE2a173976CA11".parse().unwrap()
            }
        }
    }

    /// Default PortalRegistry proxy address per network. `None` on Tethys keeps the new code
    /// path opt-in for staging until operators export `PORTAL_REGISTRY_CONTRACT_ADDR`.
    pub fn portal_registry_default_addr(&self) -> Option<Address> {
        match self {
            Self::Tethys => None,
            Self::Mainnet => Some("0x29edE9EB0ad3C02B6A98B0E41bF99Cd709812850".parse().unwrap()),
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tethys => write!(f, "tethys"),
            Self::Mainnet => write!(f, "mainnet"),
        }
    }
}
