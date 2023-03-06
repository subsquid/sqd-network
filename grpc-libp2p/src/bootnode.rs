use futures::{stream::FusedStream, StreamExt};
use libp2p::{
    autonat, identify,
    identity::Keypair,
    kad::{store::MemoryStore, Kademlia},
    relay::v2::relay::Relay,
    PeerId, Swarm,
};
use libp2p_swarm_derive::NetworkBehaviour;
use simple_logger::SimpleLogger;

#[derive(NetworkBehaviour)]
struct WorkerBehaviour {
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    autonat: autonat::Behaviour,
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

    // Prepare behaviour & transport
    let behaviour = WorkerBehaviour {
        identify: identify::Behaviour::new(identify::Config::new(
            "/subsquid/0.0.1".to_string(),
            keypair.public(),
        )),
        kademlia: Kademlia::with_config(
            local_peer_id,
            MemoryStore::new(local_peer_id),
            Default::default(),
        ),
        autonat: autonat::Behaviour::new(local_peer_id, Default::default()),
        relay: Relay::new(local_peer_id, Default::default()),
    };
    let transport = libp2p::tokio_development_transport(keypair)?;

    // Start the swarm
    let mut swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
    swarm.listen_on(listen_addr)?;
    while !swarm.is_terminated() {
        let event = swarm.select_next_some().await;
        log::debug!("Swarm event: {event:?}");
    }

    Ok(())
}
