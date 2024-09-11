use chrono::{DateTime, Utc};
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
    if let Some(sys_time) = client.worker_registration_time(cli.peer_id).await? {
        let datetime: DateTime<Utc> = sys_time.into();
        log::info!("Worker {} registered at {}", cli.peer_id, datetime.format("%d/%m/%Y %T"));
    } else {
        log::info!("Worker {} not registered", cli.peer_id);
    }

    Ok(())
}
