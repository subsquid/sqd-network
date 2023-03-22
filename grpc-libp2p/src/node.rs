use clap::Parser;
use libp2p::{
    identity::{ed25519, Keypair},
    Multiaddr, PeerId,
};
use simple_logger::SimpleLogger;
use std::{fs, path::PathBuf, str::FromStr};

#[cfg(feature = "rpc")]
use grpc_libp2p::rpc;
use grpc_libp2p::transport::P2PTransportBuilder;
#[cfg(feature = "worker")]
use grpc_libp2p::worker;

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Listen on given multiaddr (default: /ip4/0.0.0.0/tcp/0)"
    )]
    listen: Option<Option<String>>,
    #[arg(long, help = "Connect to boot node '<peer_id> <address>'.")]
    boot_nodes: Vec<BootNode>,
    #[arg(
        short,
        long,
        help = "Connect to relay. If not specified, one of the boot nodes is used."
    )]
    relay: Option<String>,
    #[arg(long, help = "Bootstrap kademlia. Makes node discoverable by others.")]
    bootstrap: bool,
    #[arg(short, long, help = "Load key from file or generate and save to file.")]
    key: Option<PathBuf>,
    #[arg(short, long, help = "Mode of operation ('worker' or 'rpc')")]
    mode: Mode,
}

#[derive(Debug, Clone)]
enum Mode {
    Worker,
    Rpc,
}

impl FromStr for Mode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "worker" => Ok(Self::Worker),
            "rpc" => Ok(Self::Rpc),
            _ => Err("Invalid mode"),
        }
    }
}

#[derive(Debug, Clone)]
struct BootNode(PeerId, Multiaddr);

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
        Ok(Self(peer_id, address))
    }
}

fn get_keypair(path: Option<PathBuf>) -> anyhow::Result<Keypair> {
    let path = match path {
        Some(path) => path,
        None => return Ok(Keypair::generate_ed25519()),
    };
    if path.is_file() {
        log::info!("Reading key from {}", path.display());
        let mut content = fs::read(&path)?;
        let keypair = ed25519::Keypair::decode(content.as_mut_slice())?;
        Ok(Keypair::Ed25519(keypair))
    } else if path.exists() {
        anyhow::bail!("Path exists and is not a file")
    } else {
        log::info!("Generating new key and saving into {}", path.display());
        let keypair = ed25519::Keypair::generate();
        fs::write(&path, keypair.encode())?;
        Ok(Keypair::Ed25519(keypair))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();
    let keypair = get_keypair(cli.key)?;
    let local_peer_id = keypair.public().to_peer_id();
    log::info!("Local peer ID: {local_peer_id}");

    // Prepare transport
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair);
    if let Some(listen_addr) = cli.listen {
        let listen_addr = listen_addr.unwrap_or("/ip4/0.0.0.0/tcp/0".to_string()).parse()?;
        transport_builder.listen_on(std::iter::once(listen_addr));
    }
    if let Some(relay_addr) = cli.relay {
        transport_builder.relay(relay_addr.parse()?);
    }
    transport_builder.boot_nodes(cli.boot_nodes.into_iter().map(|node| (node.0, node.1)));
    transport_builder.bootstrap(cli.bootstrap);

    match cli.mode {
        #[cfg(feature = "worker")]
        Mode::Worker => {
            let (msg_receiver, msg_sender) = transport_builder.run().await?;
            worker::run_worker(local_peer_id, msg_receiver, msg_sender, "".to_string()).await;
            Ok(())
        }
        #[cfg(feature = "rpc")]
        Mode::Rpc => {
            let (msg_receiver, msg_sender) = transport_builder.run().await?;
            rpc::run_server(local_peer_id, msg_receiver, msg_sender).await?;
            Ok(())
        }
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!(
            "Unsupported mode. Did you enable the appropriate feature during compilation?"
        ),
    }
}
