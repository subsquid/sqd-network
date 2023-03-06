use clap::Parser;
use cxx::let_cxx_string;
use futures::{stream::FusedStream, StreamExt};
use libp2p::identity::Keypair;
use simple_logger::SimpleLogger;

use grpc_libp2p::{
    ffi,
    transport::{read_messages, P2PTransportBuilder, Router},
    Message,
};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Listen on given multiaddr (default: /ip4/127.0.0.1/tcp/12345)"
    )]
    listen: Option<Option<String>>,
    #[arg(short, long, help = "Dial given multiaddr")]
    dial: Vec<String>,
    #[arg(
        short,
        long,
        help = "Bootstrap kademlia. Required for non-bootnodes to be discoverable."
    )]
    bootstrap: bool,
    #[arg(short, long, help = "Send messages to given peer", default_value = "")]
    send_messages: String,
    #[arg(short, long, help = "Connect to a relay node")]
    relay: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();
    let keypair = Keypair::generate_ed25519();

    // Prepare transport
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair)?;
    if let Some(listen_addr) = cli.listen {
        let listen_addr = listen_addr.unwrap_or("/ip4/127.0.0.1/tcp/12345".to_string()).parse()?;
        transport_builder.listen_on(listen_addr)?;
    }
    for relay_addr in cli.relay {
        let relay_addr = relay_addr.parse()?;
        transport_builder.add_relay(relay_addr)?;
    }
    for dial_addr in cli.dial {
        let dial_addr = dial_addr.parse()?;
        transport_builder.dial(dial_addr)?;
    }
    if cli.bootstrap {
        transport_builder.bootstrap();
    }
    let (incoming_conns, connector) = transport_builder.run();

    let sender = Router::spawn(connector);
    let sender = ffi::wrap_sender(sender.into());

    let mut worker = ffi::new_worker();
    let_cxx_string!(config = cli.send_messages);
    worker.as_mut().unwrap().configure(&config);

    let mut handler = worker.as_mut().unwrap().start(sender);
    let mut incoming_msgs = Box::pin(read_messages(incoming_conns).fuse());
    while !incoming_msgs.is_terminated() {
        let Message { peer_id, content } = incoming_msgs.select_next_some().await;
        let_cxx_string!(peer_id = peer_id.to_base58());
        handler.as_mut().unwrap().on_message_received(&peer_id, content);
    }

    Ok(())
}
