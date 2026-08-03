use std::{path::PathBuf, str::FromStr};

use libp2p::{
    identity::{ed25519, Keypair},
    multiaddr::Protocol,
    Multiaddr,
};

mod queue;
mod stream_with_payload;
mod task_manager;

pub use queue::{new_queue, Receiver, Sender};
pub use stream_with_payload::StreamWithPayload;
pub use task_manager::{CancellationToken, TaskManager, DEFAULT_SHUTDOWN_TIMEOUT};

/// Load key from file or generate and save to file.
pub async fn get_keypair(path: Option<PathBuf>) -> anyhow::Result<Keypair> {
    let Some(path) = path else {
        return Ok(Keypair::generate_ed25519());
    };
    match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_file() => {
            log::info!("Reading key from {}", path.display());
            let mut content = tokio::fs::read(&path).await?;
            let keypair = ed25519::Keypair::try_from_bytes(content.as_mut_slice())?;
            Ok(keypair.into())
        }
        Ok(_) => {
            anyhow::bail!("Path exists and is not a file")
        }
        Err(_) => {
            log::info!("Generating new key and saving into {}", path.display());
            let keypair = ed25519::Keypair::generate();
            tokio::fs::write(&path, keypair.to_bytes()).await?;
            Ok(keypair.into())
        }
    }
}

pub fn parse_env_var<T: FromStr>(var: &str, default: T) -> T {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn addr_is_reachable(addr: &Multiaddr) -> bool {
    match addr.iter().next() {
        // We need to allow private/loopback addresses for testing in local environment
        Some(Protocol::Ip4(addr)) => {
            if addr.is_loopback() {
                std::env::var("PRIVATE_NETWORK").is_ok()
            } else {
                !(addr.is_link_local())
                    && (!addr.is_private() || std::env::var("PRIVATE_NETWORK").is_ok())
            }
        }
        Some(Protocol::Ip6(addr)) => !addr.is_loopback(),
        Some(Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dnsaddr(_)) => {
            true
        }
        _ => false,
    }
}

/// `addr_is_reachable` reads `PRIVATE_NETWORK` from the environment on every call, and Rust runs
/// the tests of a binary as threads in one process — so a test that sets the variable changes what
/// every concurrently running test sees. Any test that reads or writes it must hold this lock.
#[cfg(test)]
pub(crate) fn private_network_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod test {
    use super::*;
    use libp2p::multiaddr::multiaddr;

    #[test]
    fn test_addr_is_reachable() {
        let _guard = private_network_env_lock();
        // Start from a known state: a developer's shell, or a previous test, may have set it.
        std::env::remove_var("PRIVATE_NETWORK");

        assert!(!addr_is_reachable(&multiaddr!(Ip4([127, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([169, 254, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([192, 168, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([10, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([172, 16, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip6([0, 0, 0, 0, 0, 0, 0, 1]), Tcp(12345u16))));

        std::env::set_var("PRIVATE_NETWORK", "1");

        assert!(addr_is_reachable(&multiaddr!(Ip4([127, 0, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip4([169, 254, 0, 1]), Tcp(12345u16))));
        assert!(!addr_is_reachable(&multiaddr!(Ip6([0, 0, 0, 0, 0, 0, 0, 1]), Tcp(12345u16))));

        assert!(addr_is_reachable(&multiaddr!(Ip4([192, 168, 0, 1]), Tcp(12345u16))));
        assert!(addr_is_reachable(&multiaddr!(Ip4([10, 0, 0, 1]), Tcp(12345u16))));
        assert!(addr_is_reachable(&multiaddr!(Ip4([172, 16, 0, 1]), Tcp(12345u16))));

        // Leave the process as we found it, so tests that expect private addresses to be
        // unreachable still see that.
        std::env::remove_var("PRIVATE_NETWORK");
    }
}
