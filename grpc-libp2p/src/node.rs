use clap::Parser;
use libp2p::{identity::Keypair, Multiaddr, PeerId};
use simple_logger::SimpleLogger;
use std::str::FromStr;

use grpc_libp2p::{rpc, transport::P2PTransportBuilder, worker};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Listen on given multiaddr (default: /ip4/0.0.0.0/tcp/0)"
    )]
    listen: Option<Option<String>>,
    #[arg(long, help = "Connect to boot node '<peer_id> <address>'")]
    boot_nodes: Vec<BootNode>,
    #[arg(
        short,
        long,
        help = "Connect to relay. If not specified, one of the boot nodes is used."
    )]
    relay: Option<String>,
    #[arg(
        long,
        help = "Bootstrap kademlia. Required for non-bootnodes to be discoverable."
    )]
    bootstrap: bool,
    #[arg(short, long, help = "Send messages to given peer", default_value = "")]
    send_messages: String,
    #[arg(short, long, help = "Mode of operation ('worker' or 'grpc')")]
    mode: Mode,
}

#[derive(Debug, Clone)]
enum Mode {
    Worker,
    Grpc,
}

impl FromStr for Mode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "worker" => Ok(Self::Worker),
            "grpc" => Ok(Self::Grpc),
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();
    let keypair = Keypair::generate_ed25519();
    let local_peer_id = keypair.public().to_peer_id();

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
        Mode::Worker => {
            let (msg_receiver, msg_sender) = transport_builder.run().await?;
            worker::run_worker(msg_receiver, msg_sender, cli.send_messages).await
        }
        Mode::Grpc => {
            let (msg_receiver, msg_sender) = transport_builder.run().await?;
            rpc::run_server(local_peer_id, msg_receiver, msg_sender).await?
        }
    }

    Ok(())
}
