use clap::Parser;
use env_logger::Env;

use subsquid_network_transport::{cli::TransportArgs, rpc, transport::P2PTransportBuilder};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[command(flatten)]
    transport: TransportArgs,

    #[arg(
        short,
        long,
        env = "RELAY",
        help = "Connect to relay. If address not specified, one of the boot nodes is used."
    )]
    relay: Option<Option<String>>,

    #[arg(
        long,
        env,
        default_value = "0.0.0.0:50051",
        help = "Listen address for the rpc server"
    )]
    rpc_listen_addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    // Prepare transport
    let mut transport_builder = P2PTransportBuilder::from_cli(cli.transport).await?;
    match cli.relay {
        Some(Some(addr)) => transport_builder.relay_addr(addr.parse()?),
        Some(None) => transport_builder.relay(true),
        _ => {}
    }
    let local_peer_id = transport_builder.local_peer_id();
    log::info!("Local peer ID: {local_peer_id}");

    let keypair = transport_builder.keypair();
    let (msg_receiver, transport_handle) = transport_builder.run().await?;
    rpc::run_server(keypair, cli.rpc_listen_addr, msg_receiver, transport_handle).await
}
