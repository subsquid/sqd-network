use libp2p::{identity::Keypair, multiaddr::Protocol, Multiaddr};
use std::path::PathBuf;

/// Load key from file or generate and save to file.
pub async fn get_keypair(path: Option<PathBuf>) -> anyhow::Result<Keypair> {
    let path = match path {
        Some(path) => path,
        None => return Ok(Keypair::generate_ed25519()),
    };
    match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_file() => {
            log::info!("Reading key from {}", path.display());
            let content = tokio::fs::read(&path).await?;
            let keypair = Keypair::from_protobuf_encoding(&content)?;
            Ok(keypair)
        }
        Ok(_) => {
            anyhow::bail!("Path exists and is not a file")
        }
        Err(_) => {
            log::info!("Generating new key and saving into {}", path.display());
            let keypair = Keypair::generate_ed25519();
            tokio::fs::write(&path, keypair.to_protobuf_encoding()?).await?;
            Ok(keypair)
        }
    }
}

pub fn addr_is_reachable(addr: &Multiaddr) -> bool {
    match addr.iter().next() {
        Some(Protocol::Ip4(addr)) => {
            !(addr.is_loopback() || addr.is_link_local())
            // We need to allow private addresses for testing in local environment
            &&(!addr.is_private() || std::env::var("PRIVATE_NETWORK").is_ok())
        }
        Some(Protocol::Ip6(addr)) => !addr.is_loopback(),
        Some(Protocol::Dns(_)) => true,
        Some(Protocol::Dns4(_)) => true,
        Some(Protocol::Dns6(_)) => true,
        Some(Protocol::Dnsaddr(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use libp2p::multiaddr::multiaddr;

    #[test]
    fn test_addr_is_reachable() {
        assert!(!addr_is_reachable(&multiaddr!(Ip4([127, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([169, 254, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([192, 168, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([10, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([172, 16, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip6([0, 0, 0, 0, 0, 0, 0, 1]), Tcp(12345u16))));

        std::env::set_var("PRIVATE_NETWORK", "1");

        assert!(!addr_is_reachable(&multiaddr!(Ip4([127, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([169, 254, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip6([0, 0, 0, 0, 0, 0, 0, 1]), Tcp(12345u16))));

        assert!(addr_is_reachable(&multiaddr!(Ip4([192, 168, 0, 1]), Tcp(12345u16))));
        assert!(addr_is_reachable(&multiaddr!(Ip4([10, 0, 0, 1]), Tcp(12345u16))));
        assert!(addr_is_reachable(&multiaddr!(Ip4([172, 16, 0, 1]), Tcp(12345u16))));
    }
}
