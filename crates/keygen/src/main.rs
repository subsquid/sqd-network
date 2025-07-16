use libp2p::identity::{ed25519, Keypair};
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the directory where the key will be saved
    path: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let mut path = Cli::parse().path;
    let keypair = ed25519::Keypair::generate();
    let bytes = keypair.to_bytes();
    let peer_id = Keypair::from(keypair).public().to_peer_id();
    path.push(peer_id.to_base58());
    std::fs::write(path, bytes)?;
    println!("{peer_id}");
    Ok(())
}
