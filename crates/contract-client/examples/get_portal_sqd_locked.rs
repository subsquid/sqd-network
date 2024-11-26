use clap::Parser;
use simple_logger::SimpleLogger;

use sqd_contract_client::{self, PeerId, RpcArgs};

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

    let client = sqd_contract_client::get_client(&cli.rpc).await?;

    if let Some((operator, locked)) = client.portal_sqd_locked(cli.peer_id).await? {
        log::info!("Portal {} is owned by {} and {} SQD is locked ", cli.peer_id, operator, locked,);
    } else {
        log::info!("Portal {} not registered", cli.peer_id);
    }

    Ok(())
}
