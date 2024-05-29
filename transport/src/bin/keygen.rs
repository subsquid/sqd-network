use clap::{self, command, Parser};
use libp2p::PeerId;
use std::path::PathBuf;
use subsquid_network_transport::util;

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
    let peer_id = PeerId::from_public_key(&keypair.public().into());
    println!("{peer_id}");
    Ok(())
}
