use cxx::let_cxx_string;
use grpc_libp2p::{ffi, P2PSender};

fn main() {
    let mut worker = ffi::new_worker();
    let_cxx_string!(config = "blah blah");
    worker.as_mut().unwrap().configure(&config);
    let sender = ffi::wrap_sender(Box::new(P2PSender {}));
    let mut receiver = worker.as_mut().unwrap().start(sender);
    let_cxx_string!(peer_id = "whatever");
    let buf = ffi::new_buffer(100);
    receiver.as_mut().unwrap().on_message_received(&peer_id, buf);
}
