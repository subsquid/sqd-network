use std::{path::PathBuf, time::Duration};

use clap::Parser;
use futures::{stream::FusedStream, StreamExt};
use libp2p::{
    gossipsub::{Gossipsub, MessageAuthenticity},
    identify,
    kad::{store::MemoryStore, Kademlia},
    relay::v2::relay::Relay,
    swarm::{dial_opts::DialOpts, SwarmEvent},
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use simple_logger::SimpleLogger;

use subsquid_network_transport::util::{get_keypair, BootNode};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        env,
        help = "Listening address",
        default_value = "/ip4/0.0.0.0/tcp/0"
    )]
    listen_addr: String,
    #[arg(
        short,
        long,
        env = "KEY_PATH",
        help = "Load key from file or generate and save to file."
    )]
    key: Option<PathBuf>,
    #[arg(
        long,
        env,
        help = "Connect to another boot node '<peer_id> <address>'."
    )]
    boot_nodes: Vec<BootNode>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    // autonat: autonat::Behaviour,
    relay: Relay,
    gossipsub: Gossipsub,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();
    let listen_addr = cli.listen_addr.parse()?;
    let keypair = get_keypair(cli.key).await?;
    let local_peer_id = PeerId::from(keypair.public());
    log::info!("Local peer ID: {local_peer_id}");

    // Prepare behaviour & transport
    let behaviour = Behaviour {
        identify: identify::Behaviour::new(
            identify::Config::new("/subsquid/0.0.1".to_string(), keypair.public())
                .with_interval(Duration::from_secs(60))
                .with_push_listen_addr_updates(true),
        ),
        kademlia: Kademlia::with_config(
            local_peer_id,
            MemoryStore::new(local_peer_id),
            Default::default(),
        ),
        // autonat: autonat::Behaviour::new(local_peer_id, Default::default()),
        relay: Relay::new(local_peer_id, Default::default()),
        gossipsub: Gossipsub::new(MessageAuthenticity::Signed(keypair.clone()), Default::default())
            .unwrap(),
    };
    let transport = libp2p::tokio_development_transport(keypair)?;

    // Start the swarm
    let mut swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
    log::info!("Listening on {listen_addr}");
    swarm.listen_on(listen_addr)?;

    // Connect to other boot nodes
    for BootNode { peer_id, address } in cli.boot_nodes {
        log::info!("Connecting to boot node {peer_id} at {address}");
        swarm.behaviour_mut().kademlia.add_address(&peer_id, address.clone());
        swarm.dial(DialOpts::peer_id(peer_id).addresses(vec![address]).build())?;
    }

    if let Err(_) = swarm.behaviour_mut().kademlia.bootstrap() {
        log::warn!("No peers connected. Cannot bootstrap kademlia.")
    }

    while !swarm.is_terminated() {
        let event = swarm.select_next_some().await;
        log::debug!("Swarm event: {event:?}");
        if let SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info: identify::Info { listen_addrs, .. },
        })) = event
        {
            for addr in listen_addrs {
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
            }
        }
    }

    Ok(())
}
