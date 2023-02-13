use grpc_libp2p::worker_rpc::worker_client::WorkerClient;
use grpc_libp2p::worker_rpc::HelloRequest;
use grpc_libp2p::{P2PConnector, P2PTransport};
use libp2p::identity::Keypair;
use tonic::transport::Endpoint;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    simple_logger::init_with_level(log::Level::Debug).unwrap();
    let server_peer_addr = std::env::args().nth(1).unwrap();
    let name = std::env::args().nth(2).unwrap_or("Alice".into());

    let keypair = Keypair::generate_ed25519();
    let mut transport = P2PTransport::new(keypair);
    transport.dial("/ip4/127.0.0.1/tcp/12345".parse().unwrap());
    let (_inbound, outbound_requests_sender) = transport.run();
    let connector = P2PConnector::new(outbound_requests_sender);

    // NOTE: The 'libp2p://' prefix is arbitrary, but GRPC will raise InvalidUrl error without it
    let channel = Endpoint::try_from(format!("libp2p://{}", server_peer_addr))?
        .connect_with_connector(connector)
        .await?;
    let mut client = WorkerClient::new(channel);

    loop {
        let request = tonic::Request::new(HelloRequest { name: name.clone() });
        let response = client.say_hello(request).await?;
        println!("{response:?}");
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await
    }
}
