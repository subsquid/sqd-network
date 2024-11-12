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
    let cli: Cli = Cli::parse();

    let client = sqd_contract_client::get_client(&cli.rpc).await?;
    let epoch_num = client.current_epoch().await?;
    let epoch_start = client.current_epoch_start().await?;
    let elapsed = epoch_start.elapsed()?;
    log::info!("Current epoch is {epoch_num} started {} sec ago", elapsed.as_secs());
    log::info!("Epoch length is {:?}", client.epoch_length().await?);

    Ok(())
}
