use std::{path::PathBuf, time::Duration};

use clap::Parser;
use env_logger::Env;
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, PeerId, SwarmBuilder};

use sqd_contract_client::Network;
use sqd_network_transport::{protocol::dht_protocol, util::get_keypair};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg()]
    peer: PeerId,
    #[arg(long, env, default_value_t = Network::Mainnet)]
    network: Network,
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
        .with_behaviour(|_| {
            libp2p::kad::Behaviour::with_config(
                local_peer_id,
                libp2p::kad::store::MemoryStore::new(local_peer_id),
                libp2p::kad::Config::new(dht_protocol(cli.network)),
            )
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.behaviour_mut().add_address(
        &"12D3KooW9tLMANc4Vnxp27Ypyq8m8mUv45nASahj3eSnMbGWSk1b".parse()?,
        "/dns4/mainnet.subsquid.io/udp/22445/quic-v1".parse()?,
    );

    swarm.behaviour_mut().get_closest_peers(cli.peer);

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => println!("Listening on {address:?}"),
            SwarmEvent::Behaviour(event) => {
                on_kad_event(event);
            }
            e => {
                log::debug!("Swarm event: {e:#?}");
            }
        }
    }
}

fn on_kad_event(event: libp2p::kad::Event) {
    use libp2p::kad::Event;
    match event {
        Event::OutboundQueryProgressed {
            result: result @ libp2p::kad::QueryResult::Bootstrap(_),
            ..
        } => {
            log::debug!("Outbound query progressed: {result:?}");
        }
        Event::OutboundQueryProgressed { result, .. } => {
            log::info!("Outbound query progressed: {result:?}");
        }
        Event::RoutingUpdated {
            peer, addresses, ..
        } => {
            log::info!("Routing updated: {peer} {addresses:?}");
        }
        Event::RoutablePeer { peer, address } => {
            log::info!("Routable peer: {peer} {address}");
        }
        _ => {
            log::debug!("Kademlia event: {event:#?}");
        }
    }
}
