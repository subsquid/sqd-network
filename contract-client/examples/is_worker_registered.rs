use clap::Parser;
use simple_logger::SimpleLogger;

use contract_client::{self, PeerId, RpcArgs};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    rpc: RpcArgs,
    peer_id: PeerId,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli: Cli = Cli::parse();

    let client = contract_client::get_client(&cli.rpc).await?;
    let registered = client.is_worker_registered(cli.peer_id).await?;
    log::info!("Worker {} registered? {registered}", cli.peer_id);
    Ok(())
}
