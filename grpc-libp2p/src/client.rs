use grpc_libp2p::{
    worker_rpc::{worker_client::WorkerClient, HelloRequest},
    P2PTransportBuilder,
};
use libp2p::identity::Keypair;
use simple_logger::SimpleLogger;
use tonic::transport::Endpoint;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new().env().init().unwrap();
    let server_peer_addr = std::env::args().nth(1).ok_or("Server peer ID missing")?;
    let name = std::env::args().nth(2).unwrap_or("Alice".into());

    let keypair = Keypair::generate_ed25519();
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair)?;
    transport_builder.dial("/ip4/127.0.0.1/tcp/12345".parse()?)?;
    let (_, connector) = transport_builder.run();

    // NOTE: The 'libp2p://' prefix is arbitrary, but GRPC will raise InvalidUrl error without it
    let channel = Endpoint::try_from(format!("libp2p://{server_peer_addr}"))?
        .connect_with_connector(connector)
        .await?;
    let mut client = WorkerClient::new(channel);

    loop {
        let request = tonic::Request::new(HelloRequest { name: name.clone() });
        let response = client.say_hello(request).await?.into_inner().message;
        log::info!("Received response: {response}");
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await
    }
}
