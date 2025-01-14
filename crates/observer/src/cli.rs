use clap::Parser;
use libp2p::Multiaddr;
use std::path::PathBuf;

use sqd_contract_client::Network;
use sqd_network_transport::BootNode;

#[derive(Parser)]
#[command()]
pub(crate) struct Cli {
    /// HTTP port to listen on
    #[arg(short, long, default_value_t = 8000)]
    pub(crate) port: u16,

    /// Path to libp2p key file
    #[arg(short, long, env = "KEY_PATH")]
    pub key: PathBuf,

    /// Addresses on which the p2p node will listen
    #[arg(long, env, value_delimiter = ',')]
    pub p2p_listen_addrs: Vec<Multiaddr>,

    /// Public address(es) on which the p2p node can be reached
    #[arg(long, env, value_delimiter = ',')]
    pub p2p_public_addrs: Vec<Multiaddr>,

    /// Connect to boot node '<peer_id> <address>'.
    #[arg(
            long,
            env,
            value_delimiter = ',',
            num_args = 1..,
        )]
    pub boot_nodes: Vec<BootNode>,

    /// Network to connect to (mainnet or tethys)
    #[arg(long, env, default_value_t = Network::Mainnet)]
    pub network: Network,
}
