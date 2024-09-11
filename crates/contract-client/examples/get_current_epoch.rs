use clap::Parser;
use futures::stream::StreamExt;
use simple_logger::SimpleLogger;
use std::time::Duration;

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

    client
        .epoch_stream(Duration::from_secs(10))
        .for_each(|res| {
            match res {
                Ok((epoch_num, epoch_start)) => {
                    let elapsed = epoch_start.elapsed().unwrap();
                    log::info!(
                        "Current epoch is {epoch_num} started {} sec ago",
                        elapsed.as_secs()
                    );
                }
                Err(e) => log::error!("Error getting current epoch: {e:?}"),
            }
            async {}
        })
        .await;
    Ok(())
}
