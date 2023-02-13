use libp2p::identity::Keypair;

use grpc_libp2p::P2PTransport;
use tonic::{transport::Server, Request, Response, Status};

use grpc_libp2p::worker_rpc::worker_server::{Worker, WorkerServer};
use grpc_libp2p::worker_rpc::{HelloReply, HelloRequest};

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
            message: format!("Hello {}!", request.into_inner().name).into(),
        };

        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    simple_logger::init_with_level(log::Level::Debug).unwrap();
    let keypair = Keypair::generate_ed25519();
    let mut transport = P2PTransport::new(keypair);
    transport.listen_on("/ip4/0.0.0.0/tcp/12345".parse().unwrap());
    let (incoming, _outbound_requests_sender) = transport.run();
    let worker = MyWorker::default();
    Server::builder()
        .add_service(WorkerServer::new(worker))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
