use clap::Parser;
use env_logger::Env;

use futures::StreamExt;

mod cli;
mod http_server;
mod metrics;
mod transport;

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = cli::Cli::parse();

    let mut registry = prometheus_client::registry::Registry::default();
    metrics::register_metrics(&mut registry);

    tokio::spawn(http_server::Server::new(registry).run(args.port));

    let transport = transport::Transport::build(args).await?;
    tokio::spawn(run_transport(transport));

    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down");

    Ok(())
}

async fn run_transport(mut transport: transport::Transport) -> ! {
    loop {
        match transport.select_next_some().await {
            transport::Event::Gossipsub(event) => {
                metrics::record_message(
                    event.topic,
                    event
                        .peer_id
                        .map(|peer_id| peer_id.to_string())
                        .as_deref()
                        .unwrap_or("unknown"),
                );
            }
            transport::Event::PeerSeen(event) => {
                let mut address = event.address;
                while let Some(libp2p::multiaddr::Protocol::P2p(_)) = address.iter().last() {
                    address.pop();
                }
                metrics::peer_seen(&event.peer_id.to_string(), &address.to_string());
            }
            transport::Event::WorkerHeartbeat(event) => {
                if let Some(missing) = event.heartbeat.missing_chunks {
                    metrics::worker_heartbeat(
                        &event
                            .peer_id
                            .map(|peer_id| peer_id.to_string())
                            .as_deref()
                            .unwrap_or("unknown"),
                        missing.ones(),
                    );
                } else {
                    log::warn!(
                        "Field missing_chunks is missing from the heartbeat from {:?}",
                        event.peer_id
                    );
                }
            }
            transport::Event::Ping(event) => {
                if let Ok(duration) = event.result {
                    metrics::ping(&event.peer.to_string(), duration);
                } else {
                    metrics::ping_failed(&event.peer.to_string());
                }
            }
        }
    }
}
