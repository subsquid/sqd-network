use clap::Parser;
use simple_logger::SimpleLogger;

use sqd_contract_client::{self, RpcArgs};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    rpc: RpcArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();

    let client = sqd_contract_client::get_client(&cli.rpc).await?;
    let peer_ids = client.active_portals().await?;
    peer_ids.iter().for_each(|id| println!("{id}"));
    Ok(())
}
