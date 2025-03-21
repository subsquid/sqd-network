use std::time::Duration;

use clap::Parser;
use env_logger::Env;
use futures::StreamExt;
use libp2p::{
    autonat,
    gossipsub::{self, MessageAuthenticity},
    identify,
    kad::{self, store::MemoryStore, Mode},
    ping, relay,
    swarm::SwarmEvent,
    PeerId, SwarmBuilder,
};
use libp2p_connection_limits::ConnectionLimits;
use libp2p_swarm_derive::NetworkBehaviour;
use tokio::signal::unix::{signal, SignalKind};

use sqd_network_transport::{
    get_agent_info,
    protocol::{dht_protocol, ID_PROTOCOL},
    util::{addr_is_reachable, get_keypair},
    AgentInfo, BootNode, Keypair, QuicConfig, TransportArgs, WhitelistBehavior, Wrapped,
};

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[command(flatten)]
    transport: TransportArgs,

    #[arg(long, env, value_delimiter = ',', help = "Allowed nodes")]
    allowed_nodes: Vec<PeerId>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    relay: relay::Behaviour,
    gossipsub: gossipsub::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
    conn_limits: libp2p_connection_limits::Behaviour,
    whitelist: Wrapped<WhitelistBehavior>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let listen_addrs = cli.transport.listen_addrs();
    let keypair = get_keypair(Some(cli.transport.key)).await?;
    let local_peer_id = PeerId::from(keypair.public());
    log::info!("Local peer ID: {local_peer_id}");

    let contract_client = sqd_contract_client::get_client(&cli.transport.rpc).await?;

    // Prepare behaviour & transport
    let autonat_config = autonat::Config {
        timeout: Duration::from_secs(60),
        throttle_clients_global_max: 64,
        throttle_clients_peer_max: 16,
        ..Default::default()
    };
    let mut kad_config = kad::Config::new(dht_protocol(cli.transport.rpc.network));
    kad_config.set_replication_factor(20.try_into().unwrap());
    let agent_info: AgentInfo = get_agent_info!();
    let behaviour = |keypair: &Keypair| Behaviour {
        identify: identify::Behaviour::new(
            identify::Config::new(ID_PROTOCOL.to_string(), keypair.public())
                .with_interval(Duration::from_secs(60))
                .with_push_listen_addr_updates(true)
                .with_agent_version(agent_info.to_string()),
        ),
        kademlia: kad::Behaviour::with_config(
            local_peer_id,
            MemoryStore::new(local_peer_id),
            kad_config,
        ),
        relay: relay::Behaviour::new(local_peer_id, Default::default()),
        gossipsub: gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(keypair.clone()),
            Default::default(),
        )
        .unwrap(),
        ping: ping::Behaviour::new(Default::default()),
        autonat: autonat::Behaviour::new(local_peer_id, autonat_config),
        conn_limits: libp2p_connection_limits::Behaviour::new(
            ConnectionLimits::default().with_max_established_per_peer(Some(3)),
        ),
        whitelist: WhitelistBehavior::new(contract_client, Default::default()).into(),
    };

    // Start the swarm
    let quic_config = QuicConfig::from_env();
    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic_config(|config| config.mtu_upper_bound(quic_config.mtu_discovery_max))
        .with_dns()?
        .with_behaviour(behaviour)
        .expect("infallible")
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();
    for listen_addr in listen_addrs {
        log::info!("Listening on {}", listen_addr);
        swarm.listen_on(listen_addr)?;
    }
    for public_addr in cli.transport.p2p_public_addrs {
        log::info!("Adding public address {public_addr}");
        swarm.add_external_address(public_addr);
    }

    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

    // Connect to other boot nodes
    for BootNode { peer_id, address } in cli
        .transport
        .boot_nodes
        .into_iter()
        .filter(|node| node.peer_id != local_peer_id)
    {
        log::info!("Connecting to boot node {peer_id} at {address}");
        swarm.behaviour_mut().whitelist.allow_peer(peer_id);
        swarm.behaviour_mut().kademlia.add_address(&peer_id, address);
        swarm.dial(peer_id)?;
    }

    for peer_id in cli.allowed_nodes {
        log::info!("Adding allowed peer {peer_id}");
        swarm.behaviour_mut().whitelist.allow_peer(peer_id);
    }

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    loop {
        let event = tokio::select! {
            event = swarm.select_next_some() => event,
            _ = sigint.recv() => break,
            _ = sigterm.recv() => break,
        };
        log::trace!("Swarm event: {event:?}");
        if let SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info: identify::Info { listen_addrs, .. },
            ..
        })) = event
        {
            listen_addrs.into_iter().filter(addr_is_reachable).for_each(|addr| {
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
            });
        }
    }

    log::info!("Bootnode stopped");
    Ok(())
}
