use libp2p::identity::Keypair;
use simple_logger::SimpleLogger;

use grpc_libp2p::P2PTransportBuilder;
use tonic::{transport::Server, Request, Response, Status};

use grpc_libp2p::worker_rpc::{
    worker_server::{Worker, WorkerServer},
    HelloReply, HelloRequest,
};

#[derive(Debug, Default)]
pub struct MyWorker {}

#[tonic::async_trait]
impl Worker for MyWorker {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        log::info!("Got a request: {request:?}");

        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };

        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new().env().init().unwrap();
    let keypair = Keypair::generate_ed25519();
    let mut transport_builder = P2PTransportBuilder::from_keypair(keypair)?;
    transport_builder.listen_on("/ip4/0.0.0.0/tcp/12345".parse()?)?;
    let (incoming, _) = transport_builder.run();
    let worker = MyWorker::default();
    Server::builder()
        .add_service(WorkerServer::new(worker))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
