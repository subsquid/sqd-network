use libp2p::{identity::ed25519, PeerId};
use std::{io::Write, path::PathBuf};
use clap::{self, Parser, command};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the generated key
    filename: PathBuf,
}

fn main() -> std::io::Result<()> {
    let filename = Cli::parse().filename;
    let keypair;
    if let Ok(mut key) = std::fs::read(&filename) {
        keypair = ed25519::Keypair::try_from_bytes(&mut key).unwrap();
    } else {
        keypair = ed25519::Keypair::generate();
    }
    let peer_id = PeerId::from_public_key(&keypair.public().into());
    println!("{peer_id}");
    std::fs::File::create(filename).unwrap().write_all(&keypair.to_bytes())?;
    Ok(())
}
