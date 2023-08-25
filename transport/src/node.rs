use std::str::FromStr;

use clap::Parser;
use env_logger::Env;

#[cfg(feature = "rpc")]
use subsquid_network_transport::rpc;
#[cfg(feature = "worker")]
use subsquid_network_transport::worker;
use subsquid_network_transport::{cli::TransportArgs, transport::P2PTransportBuilder};

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
        short,
        long,
        default_value = "rpc",
        help = "Mode of operation ('worker' or 'rpc')"
    )]
    mode: Mode,

    #[arg(
        long,
        env,
        default_value = "0.0.0.0:50051",
        help = "Listen address for the rpc server"
    )]
    rpc_listen_addr: String,
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

    match cli.mode {
        #[cfg(feature = "worker")]
        Mode::Worker => {
            let (msg_receiver, msg_sender, _) = transport_builder.run().await?;
            worker::run_worker(local_peer_id, msg_receiver, msg_sender, "".to_string()).await;
            Ok(())
        }
        #[cfg(feature = "rpc")]
        Mode::Rpc => {
            let (msg_receiver, msg_sender, subscription_sender) = transport_builder.run().await?;
            rpc::run_server(
                local_peer_id,
                cli.rpc_listen_addr,
                msg_receiver,
                msg_sender,
                subscription_sender,
            )
            .await?;
            Ok(())
        }
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!(
            "Unsupported mode. Did you enable the appropriate feature during compilation?"
        ),
    }
}
