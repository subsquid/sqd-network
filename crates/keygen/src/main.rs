use std::path::PathBuf;

use clap::Parser;

use sqd_network_transport::util;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the generated key
    filename: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filename = Cli::parse().filename;
    let keypair = util::get_keypair(Some(filename)).await?;
    let peer_id = keypair.public().to_peer_id();
    println!("{peer_id}");
    Ok(())
}
