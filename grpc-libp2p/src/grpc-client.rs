use clap::Parser;
use simple_logger::SimpleLogger;
use std::time::Duration;
use tokio::time::Instant;
use tokio_stream::StreamExt;

use grpc_libp2p::rpc::api::{
    p2p_transport_client::P2pTransportClient, Empty, Message, Subscription,
};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    server_addr: String,
    #[arg(short, long)]
    peer_id: Option<String>,
    #[arg(short, long)]
    topic: Option<String>,
    #[arg(short, long)]
    subscribe: Vec<String>,
    #[arg(short, long)]
    echo: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();

    let mut client = P2pTransportClient::connect(cli.server_addr).await?;
    let local_peer_id = client.local_peer_id(Empty {}).await?.into_inner().peer_id;
    log::info!("Local peer ID: {local_peer_id}");

    if cli.peer_id.is_some() || cli.topic.is_some() {
        let msg = Message {
            peer_id: cli.peer_id,
            content: "Hello!".as_bytes().to_vec(),
            topic: cli.topic,
        };

        let mut incoming = client.get_messages(Empty {}).await?.into_inner();
        loop {
            log::info!("Sending message {msg:?}");
            let start = Instant::now();
            let _ = client
                .send_message(msg.clone())
                .await
                .map_err(|e| log::error!("Error sending message: {e:?}"));

            match tokio::time::timeout(Duration::from_secs(1), incoming.next()).await {
                Ok(reply) => log::info!("Got reply: {:?}", reply.unwrap()?),
                Err(_) => log::error!("Timeout"),
            };
            let duration = start.elapsed();
            log::info!("Message round trip time: {duration:?}");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    } else {
        for topic in cli.subscribe {
            log::info!("Subscribing to topic {topic}");
            let sub = Subscription {
                topic,
                subscribed: true,
            };
            client.toggle_subscription(sub).await?;
        }

        let mut incoming = client.get_messages(Empty {}).await?.into_inner();
        while let Some(Ok(mut msg)) = incoming.next().await {
            let peer_id = (&msg.peer_id).as_ref().ok_or(anyhow::anyhow!("Peer ID missing"))?;
            let topic = (&msg.topic).as_ref().map(|s| s.as_str()).unwrap_or("<no topic>");
            log::info!(
                "Message from peer {peer_id} (topic {topic}): {}",
                String::from_utf8_lossy(&msg.content)
            );
            if cli.echo {
                msg.topic = None; // Always respond with direct msg, not broadcast
                let _ = client
                    .send_message(msg)
                    .await
                    .map_err(|e| log::error!("Error sending message: {e:?}"));
            }
        }
    }
    Ok(())
}
