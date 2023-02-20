use api::{worker_server, HelloReply, HelloRequest};
use tonic::{Request, Response, Status};

pub mod api {
    tonic::include_proto!("worker_rpc"); // The string specified here must match the proto package name
}

#[derive(Debug, Default)]
pub struct Worker {}

#[tonic::async_trait]
impl worker_server::Worker for Worker {
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
