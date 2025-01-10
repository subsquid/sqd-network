use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use clap::Parser;
use env_logger::Env;
use futures::StreamExt;
use lazy_static::lazy_static;
use libp2p::{
    gossipsub::{Sha256Topic, TopicHash},
    identity::Keypair,
    swarm::{behaviour::toggle::Toggle, SwarmEvent},
    Multiaddr, PeerId, SwarmBuilder,
};

use sqd_contract_client::Network;
use sqd_network_transport::{
    protocol::{self, dht_protocol},
    util::get_keypair,
};

#[derive(Parser)]
#[command()]
struct Cli {
    #[arg(long, env = "KEY_PATH", help = "Path to libp2p key file")]
    key: PathBuf,

    /// The network to use
    #[arg(long, default_value_t = Network::Mainnet)]
    network: Network,

    /// The port to listen on (may be 0 to select randomly)
    #[arg(short, long)]
    listen: Option<u16>,

    /// Dial the specified addresses
    #[arg(short, long)]
    dial: Vec<Multiaddr>,

    /// Enable the Ping protocol
    #[arg(short, long)]
    ping: bool,

    /// Enable the Identify protocol
    #[arg(short, long)]
    identify: bool,

    /// Enable the Kademlia protocol
    #[arg(short, long)]
    kad: bool,

    /// Enable the AutoNAT protocol
    #[arg(short, long)]
    autonat: bool,

    /// Enable the Gossipsub protocol
    #[arg(short, long)]
    gossipsub: bool,
}

#[derive(libp2p_swarm_derive::NetworkBehaviour)]
struct Behaviour {
    ping: Toggle<libp2p::ping::Behaviour>,
    identify: Toggle<libp2p::identify::Behaviour>,
    autonat: Toggle<libp2p::autonat::Behaviour>,
    kad: Toggle<libp2p::kad::Behaviour<libp2p::kad::store::MemoryStore>>,
    gossipsub: Toggle<libp2p::gossipsub::Behaviour>,
}

impl Behaviour {
    fn new(key: &Keypair, cli: &Cli) -> Self {
        let local_peer_id = PeerId::from(key.public());

        let ping_enabled = cli.ping;
        let identify_enabled = cli.identify | cli.autonat;
        let autonat_enabled = cli.autonat;
        let kad_enabled = cli.kad | cli.gossipsub;
        let gossipsub_enabled = cli.gossipsub;

        let autonat_config = libp2p::autonat::Config {
            boot_delay: Duration::from_secs(1),
            ..Default::default()
        };
        let gossipsub_config = libp2p::gossipsub::ConfigBuilder::default()
            .validation_mode(libp2p::gossipsub::ValidationMode::Permissive)
            .build()
            .unwrap();

        Self {
            ping: ping_enabled.then(|| Default::default()).into(),
            identify: identify_enabled
                .then(|| {
                    libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                        protocol::ID_PROTOCOL.to_owned(),
                        key.public(),
                    ))
                })
                .into(),
            autonat: autonat_enabled
                .then(|| libp2p::autonat::Behaviour::new(local_peer_id, autonat_config))
                .into(),
            gossipsub: gossipsub_enabled
                .then(|| {
                    libp2p::gossipsub::Behaviour::new(
                        libp2p::gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossipsub_config,
                    )
                    .unwrap()
                })
                .into(),
            kad: kad_enabled
                .then(|| {
                    libp2p::kad::Behaviour::with_config(
                        local_peer_id,
                        libp2p::kad::store::MemoryStore::new(local_peer_id),
                        libp2p::kad::Config::new(dht_protocol(cli.network)),
                    )
                })
                .into(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let keypair = get_keypair(Some(cli.key.clone())).await?;
    let local_peer_id = PeerId::from(keypair.public());
    log::info!("Local peer ID: {local_peer_id}");

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_dns()?
        .with_behaviour(|key| Behaviour::new(key, &cli))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    if let Some(kad) = swarm.behaviour_mut().kad.as_mut() {
        for (addr, peer_id) in bootnodes(cli.network) {
            kad.add_address(&peer_id, addr);
        }
    }

    if let Some(autonat) = swarm.behaviour_mut().autonat.as_mut() {
        for (addr, peer_id) in bootnodes(cli.network) {
            autonat.add_server(peer_id, Some(addr));
        }
    }

    if let Some(port) = cli.listen {
        swarm.listen_on(format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?)?;
    }

    for addr in cli.dial {
        log::info!("Dialing {addr}");
        swarm.dial(addr)?;
    }

    if let Some(behaviour) = swarm.behaviour_mut().gossipsub.as_mut() {
        for topic_name in *KNOWN_TOPICS {
            let topic = Sha256Topic::new(topic_name);
            behaviour.subscribe(&topic)?;
        }
    }

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => log::info!("Listening on {address:?}"),
            SwarmEvent::Behaviour(event) => match event {
                BehaviourEvent::Ping(e) => log::info!("Ping event: {e:?}"),
                BehaviourEvent::Autonat(e) => log::info!("AutoNAT event: {e:?}"),
                BehaviourEvent::Gossipsub(libp2p::gossipsub::Event::Message {
                    message, ..
                }) => on_gossipsub_message(message),
                BehaviourEvent::Gossipsub(e) => {
                    log::info!("Gossipsub event: {e:?}");
                }
                BehaviourEvent::Identify(e) => {
                    let level = if cli.identify {
                        log::Level::Info
                    } else {
                        log::Level::Debug
                    };
                    log::log!(level, "Identify event: {e:?}")
                }
                BehaviourEvent::Kad(e) => {
                    let level = if cli.kad {
                        log::Level::Info
                    } else {
                        log::Level::Debug
                    };
                    log::log!(level, "Kademlia event: {e:?}");
                }
            },
            e => {
                log::debug!("Swarm event: {e:?}");
            }
        }
    }
}

fn bootnodes(network: Network) -> impl Iterator<Item = (Multiaddr, PeerId)> {
    match network {
        Network::Mainnet => vec![
            (
                "/dns4/mainnet.subsquid.io/udp/22445/quic-v1".parse().unwrap(),
                "12D3KooW9tLMANc4Vnxp27Ypyq8m8mUv45nASahj3eSnMbGWSk1b".parse().unwrap(),
            ),
            (
                "/dns4/mainnet.subsquid.io/udp/22446/quic-v1".parse().unwrap(),
                "12D3KooWEhPC7rsHAcifstVwJ3Cj55sWn7zXWuHrtAQUCGhGYnQz".parse().unwrap(),
            ),
            (
                "/dns4/mainnet.subsquid.io/udp/32445/quic-v1".parse().unwrap(),
                "12D3KooWS5N8ygU6fRy4EZtzdHf4QZnkCaZrwCha9eYKH3LwNvsP".parse().unwrap(),
            ),
        ]
        .into_iter(),
        Network::Tethys => vec![
            (
                "/dns4/testnet.subsquid.io/udp/22445/quic-v1".parse().unwrap(),
                "12D3KooWSRvKpvNbsrGbLXGFZV7GYdcrYNh4W2nipwHHMYikzV58".parse().unwrap(),
            ),
            (
                "/dns4/testnet.subsquid.io/udp/22446/quic-v1".parse().unwrap(),
                "12D3KooWQC9tPzj2ShLn39RFHS5SGbvbP2pEd7bJ61kSW2LwxGSB".parse().unwrap(),
            ),
        ]
        .into_iter(),
    }
}

fn on_gossipsub_message(message: libp2p::gossipsub::Message) {
    use sqd_messages::ProstMsg;

    lazy_static! {
        static ref TOPIC_BY_HASH: BTreeMap<TopicHash, &'static str> =
            BTreeMap::from_iter(KNOWN_TOPICS.iter().map(|&topic| (topic_hash(topic), topic)));
    }

    let Some(topic) = TOPIC_BY_HASH.get(&message.topic) else {
        log::error!("Unknown topic hash: {}", message.topic);
        return;
    };

    match topic {
        &protocol::HEARTBEAT_TOPIC => {
            let peer_id = message.source.expect("message should have a known source");
            match sqd_messages::Heartbeat::decode(message.data.as_slice()) {
                Ok(heartbeat) => {
                    log::info!("Gossipsub message ({topic}) from {peer_id}: {heartbeat:?}")
                }
                Err(e) => log::warn!("Couldn't parse heartbeat from {peer_id}: {e:?}",),
            }
        }
        topic => {
            log::info!("Gossipsub legacy message ({topic}): {message:?}");
            return;
        }
    }
}

fn topic_hash(topic: &str) -> TopicHash {
    Sha256Topic::new(topic).hash()
}

const WORKER_LOGS_TOPIC_1_0: &str = "/subsquid/worker_query_logs/1.0.0";
const WORKER_LOGS_TOPIC_1_1: &str = "/subsquid/worker_query_logs/1.1.0";
const LOGS_COLLECTED_TOPIC: &str = "/subsquid/logs_collected/1.0.0";

lazy_static! {
    static ref KNOWN_TOPICS: [&'static str; 5] = [
        protocol::HEARTBEAT_TOPIC,
        protocol::OLD_PING_TOPIC,
        WORKER_LOGS_TOPIC_1_0,
        WORKER_LOGS_TOPIC_1_1,
        LOGS_COLLECTED_TOPIC,
    ];
}
