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
    let libp2p_metrics = libp2p::metrics::Metrics::new(&mut registry);

    tokio::spawn(http_server::Server::new(registry).run(args.port));

    let transport = transport::Transport::build(args, libp2p_metrics).await?;
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
                let peer_id =
                    event.peer_id.map(|peer_id| peer_id.to_string()).unwrap_or_else(|| {
                        log::warn!("Received heartbeat from unknown peer");
                        "unknown".to_string()
                    });

                let missing_chunks = event
                    .heartbeat
                    .missing_chunks
                    .map(|bitstring| bitstring.ones())
                    .unwrap_or_else(|| {
                        log::warn!("Received heartbeat without missing_chunks from {peer_id}");
                        0
                    });

                let assignment_time = chrono::NaiveDateTime::parse_and_remainder(
                    &event.heartbeat.assignment_id,
                    "%Y-%m-%dT%H:%M:%S",
                )
                .map(|(time, _)| time.and_utc().timestamp())
                .unwrap_or_else(|e| {
                    log::warn!(
                        "Failed to parse assignment_id '{}': {e}",
                        &event.heartbeat.assignment_id
                    );
                    0
                });

                metrics::worker_heartbeat(
                    &peer_id,
                    missing_chunks,
                    event.heartbeat.stored_bytes.unwrap_or_default(),
                    assignment_time,
                );
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
