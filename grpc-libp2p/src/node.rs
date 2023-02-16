use clap::Parser;
use futures::TryStreamExt;
use libp2p::identity::Keypair;
use simple_logger::SimpleLogger;
use tokio::io;
use tokio_util::codec::{FramedRead, LinesCodec};
use tonic::{
    transport::{Endpoint, Server},
    Request,
};

use grpc_libp2p::{
    transport::{P2PConnector, P2PTransportBuilder},
    worker::Worker,
    worker_api::{worker_client::WorkerClient, worker_server::WorkerServer, HelloRequest},
};

#[derive(Parser)]
#[command(version, author)]
struct Cli {
    #[arg(
        short,
        long,
        help = "Listen on given multiaddr (default: /ip4/127.0.0.1/tcp/12345)"
    )]
    listen: Option<Option<String>>,
    #[arg(short, long, help = "Dial given multiaddr")]
    dial: Vec<String>,
}

async fn make_request(connector: P2PConnector, params: String) -> anyhow::Result<()> {
    // The params are expected to be "<peer_id> <whatever>"
    let mut params = params.split_whitespace();
    let peer_id = params
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to read peer ID"))?
        .to_string();
    let name = params.next().ok_or_else(|| anyhow::anyhow!("Failed to read name"))?.to_string();
    let message = HelloRequest { name };

    // NOTE: The 'libp2p://' prefix is arbitrary, but GRPC will raise InvalidUrl error without it
    let channel = Endpoint::try_from(format!("libp2p://{peer_id}"))?
        .connect_with_connector(connector.clone())
        .await?;
    let mut client = WorkerClient::new(channel);

    let request = Request::new(message.clone());
    let response = client.say_hello(request).await?.into_inner().message;
    log::info!("Received response: {response}");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging and parse arguments
    SimpleLogger::new().with_level(log::LevelFilter::Info).env().init()?;
    let cli = Cli::parse();
    let keypair = Keypair::generate_ed25519();

    // Prepare transport
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair)?;
    if let Some(listen_addr) = cli.listen {
        let listen_addr = listen_addr.unwrap_or("/ip4/127.0.0.1/tcp/12345".to_string()).parse()?;
        transport_builder.listen_on(listen_addr)?;
    }
    for dial_addr in cli.dial {
        let dial_addr = dial_addr.parse()?;
        transport_builder.dial(dial_addr)?;
    }
    let (incoming, connector) = transport_builder.run();

    // Serve incoming requests in the background
    let worker = Worker::default();
    tokio::task::spawn(
        Server::builder()
            .add_service(WorkerServer::new(worker))
            .serve_with_incoming(incoming),
    );

    // Read messages from stdin and send to peers
    let stdin = io::stdin();
    let reader = FramedRead::new(stdin, LinesCodec::new());
    reader
        .map_err(|e| anyhow::anyhow!("Codec error: {e:?}"))
        .try_for_each(|line| {
            let connector = connector.clone();
            async move {
                let _ = make_request(connector, line).await.map_err(|e| log::error!("{e}"));
                Ok(())
            }
        })
        .await?;

    Ok(())
}
