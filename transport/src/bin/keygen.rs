use libp2p::{identity::ed25519, PeerId};
use std::io::Write;

fn main() -> std::io::Result<()> {
    let keypair = ed25519::Keypair::generate();
    let peer_id = PeerId::from_public_key(&keypair.public().into());
    eprintln!("Your peer ID: {peer_id}");
    std::io::stdout().write_all(&keypair.to_bytes())?;
    Ok(())
}
