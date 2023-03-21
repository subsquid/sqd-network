use grpc_libp2p::rpc::api::{p2p_transport_client::P2pTransportClient, Empty, Message};
use simple_logger::SimpleLogger;
use std::time::Duration;
use tokio::time::Instant;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let mut args: Vec<String> = std::env::args().collect();

    let mut client = P2pTransportClient::connect("http://127.0.0.1:50051").await?;
    let local_peer_id = client.local_peer_id(Empty {}).await?.into_inner().peer_id;
    log::info!("Local peer ID: {local_peer_id}");

    if args.len() > 1 {
        let peer_id = args.pop().unwrap();
        let mut incoming = client.get_messages(Empty {}).await?.into_inner();
        loop {
            log::info!("Sending message to {peer_id}");
            let content = "Hello!".as_bytes().to_vec();
            let msg = Message {
                peer_id: peer_id.clone(),
                content,
            };
            let start = Instant::now();
            let _ = client
                .send_message(msg)
                .await
                .map_err(|e| log::error!("Error sending message: {e:?}"));
            let reply = incoming.next().await.unwrap()?;
            let duration = start.elapsed();
            log::info!("Message round trip time: {duration:?}");
            anyhow::ensure!(reply.peer_id == peer_id);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    } else {
        let mut incoming = client.get_messages(Empty {}).await?.into_inner();
        while let Some(msg) = incoming.next().await {
            let Message { peer_id, content } = msg?;
            log::info!("Message from peer {peer_id}: {}", String::from_utf8_lossy(&content));
        }
    }
    Ok(())
}
