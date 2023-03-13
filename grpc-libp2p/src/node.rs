use clap::Parser;
use cxx::let_cxx_string;
use futures::{stream::FusedStream, StreamExt};
use libp2p::{identity::Keypair, Multiaddr, PeerId};
use simple_logger::SimpleLogger;
use std::str::FromStr;

use grpc_libp2p::{ffi, transport::P2PTransportBuilder, Message};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Listen on given multiaddr (default: /ip4/127.0.0.1/tcp/12345)"
    )]
    listen: Option<Option<String>>,
    #[arg(long, help = "Connect to boot node '<peer_id> <address>'")]
    boot_nodes: Vec<BootNode>,
    #[arg(
        long,
        help = "Bootstrap kademlia. Required for non-bootnodes to be discoverable."
    )]
    bootstrap: bool,
    #[arg(short, long, help = "Send messages to given peer", default_value = "")]
    send_messages: String,
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

    // Prepare transport
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair);
    if let Some(listen_addr) = cli.listen {
        let listen_addr = listen_addr.unwrap_or("/ip4/127.0.0.1/tcp/12345".to_string()).parse()?;
        transport_builder.listen_on(std::iter::once(listen_addr));
    }
    transport_builder.boot_nodes(cli.boot_nodes.into_iter().map(|node| (node.0, node.1)));
    transport_builder.bootstrap(cli.bootstrap);
    let (mut inbound_messages, sender) = transport_builder.run().await?;

    let sender = ffi::wrap_sender(sender.into());

    let mut worker = ffi::new_worker();
    let_cxx_string!(config = cli.send_messages);
    worker.as_mut().unwrap().configure(&config);

    let mut handler = worker.as_mut().unwrap().start(sender);
    while !inbound_messages.is_terminated() {
        let Message { peer_id, content } = inbound_messages.select_next_some().await;
        let_cxx_string!(peer_id = peer_id.to_base58());
        handler.as_mut().unwrap().on_message_received(&peer_id, content);
    }

    Ok(())
}
