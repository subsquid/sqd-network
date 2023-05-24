use libp2p::{
    identity::{ed25519, Keypair},
    Multiaddr, PeerId,
};
use std::{path::PathBuf, str::FromStr};

/// Load key from file or generate and save to file.
pub async fn get_keypair(path: Option<PathBuf>) -> anyhow::Result<Keypair> {
    let path = match path {
        Some(path) => path,
        None => return Ok(Keypair::generate_ed25519()),
    };
    match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_file() => {
            log::info!("Reading key from {}", path.display());
            let mut content = tokio::fs::read(&path).await?;
            let keypair = ed25519::Keypair::decode(content.as_mut_slice())?;
            Ok(Keypair::Ed25519(keypair))
        }
        Ok(_) => {
            anyhow::bail!("Path exists and is not a file")
        }
        Err(_) => {
            log::info!("Generating new key and saving into {}", path.display());
            let keypair = ed25519::Keypair::generate();
            tokio::fs::write(&path, keypair.encode()).await?;
            Ok(Keypair::Ed25519(keypair))
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootNode {
    pub peer_id: PeerId,
    pub address: Multiaddr,
}

impl FromStr for BootNode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let peer_id = parts
            .next()
            .ok_or("Boot node peer ID missing")?
            .parse()
            .map_err(|_| "Invalid peer ID")?;
        let address = parts
            .next()
            .ok_or("Boot node address missing")?
            .parse()
            .map_err(|_| "Invalid address")?;
        Ok(Self { peer_id, address })
    }
}
