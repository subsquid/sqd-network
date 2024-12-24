use std::{path::PathBuf, time::Duration};

use clap::Parser;
use env_logger::Env;
use futures::StreamExt;
use libp2p::{identity::Keypair, swarm::SwarmEvent, PeerId, SwarmBuilder};

use sqd_network_transport::{protocol, util::get_keypair};

#[derive(Parser)]
#[command()]
struct Cli {
    /// Path to libp2p key file
    #[arg(short, long, env = "KEY_PATH")]
    pub key: PathBuf,

    /// Optional port to listen on
    #[arg(short, long, env, default_value_t = 0)]
    pub port: u16,
}

#[derive(libp2p_swarm_derive::NetworkBehaviour)]
struct Behaviour {
    identify: libp2p::identify::Behaviour,
    autonat: libp2p::autonat::Behaviour,
}

impl Behaviour {
    fn new(key: &Keypair) -> Self {
        let autonat_config = libp2p::autonat::Config {
            boot_delay: Duration::from_secs(1),
            ..Default::default()
        };
        Self {
            identify: libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                protocol::ID_PROTOCOL.to_owned(),
                key.public(),
            )),
            autonat: libp2p::autonat::Behaviour::new(key.public().to_peer_id(), autonat_config),
        }
    }
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
        .with_behaviour(Behaviour::new)?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{}/quic-v1", cli.port).parse()?)?;
    swarm.behaviour_mut().autonat.add_server(
        "12D3KooW9tLMANc4Vnxp27Ypyq8m8mUv45nASahj3eSnMbGWSk1b".parse()?,
        Some("/dns4/mainnet.subsquid.io/udp/22445/quic-v1".parse()?),
    );

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => println!("Listening on {address:?}"),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
                log::info!("Identify event: {event:?}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::Autonat(event)) => {
                log::info!("Autonat event: {event:?}");
            }
            e => {
                log::info!("Swarm event: {e:?}");
            }
        }
    }
}
