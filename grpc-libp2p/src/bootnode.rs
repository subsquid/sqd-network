use futures::{stream::FusedStream, StreamExt};
use libp2p::{
    identify,
    identity::Keypair,
    kad::{store::MemoryStore, Kademlia},
    relay::v2::relay::Relay,
    swarm::SwarmEvent,
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use simple_logger::SimpleLogger;
use std::time::Duration;

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    // autonat: autonat::Behaviour,
    relay: Relay,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let args: Vec<String> = std::env::args().collect();
    let listen_addr = args[1].parse()?;
    let keypair = Keypair::generate_ed25519();
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
    };
    // let behaviour = Relay::new(local_peer_id, Default::default());
    let transport = libp2p::tokio_development_transport(keypair)?;

    // Start the swarm
    let mut swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
    swarm.listen_on(listen_addr)?;
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
