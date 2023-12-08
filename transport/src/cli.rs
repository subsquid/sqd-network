use crate::PeerId;
use clap::Args;
use libp2p::Multiaddr;
use std::{path::PathBuf, str::FromStr};

#[derive(Args)]
pub struct TransportArgs {
    #[arg(short, long, env = "KEY_PATH", help = "Path to libp2p key file")]
    pub key: Option<PathBuf>,

    #[arg(
        long,
        env,
        help = "Address on which the p2p node will listen",
        default_value = "/ip4/0.0.0.0/tcp/0"
    )]
    pub p2p_listen_addr: Multiaddr,

    #[arg(
        long,
        env,
        help = "Public address(es) on which the p2p node can be reached",
        value_delimiter = ',',
        num_args = 1..,
    )]
    pub p2p_public_addrs: Vec<Multiaddr>,

    #[arg(
        long,
        env,
        help = "Connect to boot node '<peer_id> <address>'.",
        value_delimiter = ',',
        num_args = 1..,
    )]
    pub boot_nodes: Vec<BootNode>,

    #[arg(
        long,
        env,
        help = "Bootstrap kademlia. Makes node discoverable by others."
    )]
    pub bootstrap: bool,
}

#[derive(Debug, Clone)]
pub struct BootNode {
    pub peer_id: PeerId,
    pub address: Multiaddr,
}

impl FromStr for BootNode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let peer_id = parts
            .next()
            .ok_or("Boot node peer ID missing")?
            .parse()
            .map_err(|_| "Invalid peer ID")?;
        let address = parts
            .next()
            .ok_or("Boot node address missing")?
            .parse()
            .map_err(|_| "Invalid address")?;
        Ok(Self { peer_id, address })
    }
}
