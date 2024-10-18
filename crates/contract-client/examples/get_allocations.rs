use clap::Parser;
use simple_logger::SimpleLogger;

use sqd_contract_client::{self, PeerId, RpcArgs};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    rpc: RpcArgs,
    client_id: PeerId,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli: Cli = Cli::parse();

    let client = sqd_contract_client::get_client(&cli.rpc).await?;
    let worker_id = client.worker_id(cli.client_id).await?;
    let allocations = client.gateway_clusters(worker_id).await?;
    allocations.iter().for_each(|cluster| println!("{cluster:?}"));
    Ok(())
}
