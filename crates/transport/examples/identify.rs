use std::{path::PathBuf, str::FromStr, time::Duration};

use clap::Parser;
use env_logger::Env;
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, SwarmBuilder};

use sqd_network_transport::{protocol, util::get_keypair};

#[derive(Parser)]
#[command()]
struct Cli {
    #[arg()]
    remote: Multiaddr,
    #[arg(short, long, env = "KEY_PATH", help = "Path to libp2p key file")]
    pub key: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let keypair = get_keypair(Some(cli.key)).await?;
    let local_peer_id = PeerId::from(keypair.public());
    log::info!("Local peer ID: {local_peer_id}");

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_dns()?
        .with_behaviour(|key| {
            libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                protocol::ID_PROTOCOL.to_owned(),
                key.public(),
            ))
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.dial(cli.remote)?;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => println!("Listening on {address:?}"),
            SwarmEvent::Behaviour(event) => {
                log::info!("Identify event: {event:?}");
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                log::info!("Connected to {peer_id}");
            }
            e => {
                log::info!("Swarm event: {e:?}");
            }
        }
    }
}
