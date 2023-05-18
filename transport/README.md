# Subsquid network

This is a libp2p-based network which allows to send arbitrary messages between nodes.

## Binaries
1. `bootnode` – A simple node which doesn't do any meaningful work, but only acts as relay server and a source of information about other nodes.
2. `node` – The proper node which can operate in one of two modes: `worker` (executing worker logic provided as a C++ library) or `rpc` (forwarding messages to/from another process, e.g. query planner). For more details see further.
3. `grpc-client` – A binary for testing which connects to a local `node` running in `rpc` mode and sends/receives some test messages.

## Running the node

```
Usage: node [OPTIONS] --mode <MODE>

Options:
  -l, --listen [<LISTEN>]        Listen on given multiaddr (default: /ip4/0.0.0.0/tcp/0)
      --boot-nodes <BOOT_NODES>  Connect to boot node '<peer_id> <address>'.
  -r, --relay <RELAY>            Connect to relay. If not specified, one of the boot nodes is used.
      --bootstrap                Bootstrap kademlia. Makes node discoverable by others.
  -k, --key <KEY>                Load key from file or generate and save to file.
  -m, --mode <MODE>              Mode of operation ('worker' or 'rpc')
  -h, --help                     Print help
  -V, --version                  Print version
```

In `worker` mode the node executes worker logic provided by `RustBinding` library from `sql-archives` worker (currently this is just a dummy echo worker, not the real one). To use this mode, use `--features worker` flag for compiling `node`.

In `rpc` mode the node starts a gRPC server allowing to send and receive messages to/from the network by another process. The server listens on `0.0.0.0:50051` (not configurable). To use this mode, use `--features rpc` flag for compiling `node`. The server API is described below.
```protobuf
syntax = "proto3";
package p2p_transport;

service P2PTransport {
  rpc LocalPeerId(Empty) returns (PeerId);
  rpc GetMessages(Empty) returns (stream Message);
  rpc SendMessage(Message) returns (Empty);
}

message Empty {}

message PeerId {
  string peer_id = 1;
}

message Message {
  string peer_id = 1;
  bytes content = 2;
}
```

## Docker images
There is a Dockerfile provided which allows to build 3 images:
1. `docker build --target worker` – node with `worker` mode enabled
2. `docker build --target rpc-server` – node with `rpc` mode enabled
3. `docker build --target bootnode` – bootnode

## Remaining features / issues

* [ ] Put actual worker logic in the `RustBinding` library. Currently, it's just a dummy implementation which sends back received messages ([rust_binding.cpp](https://github.com/subsquid/sql-archives/blob/wiezzel/test-worker/worker/src/rust_binding.cpp)). This requires message de-multiplexing and serialization/deserialization to be implemented.
* [ ] Get rid of explicitly linking `fmt` and `spodlog` in `build.rs`. These are dependencies of `RustBinsing`. It shouldn't be necessary to enumerate them when linking with that library. The code fails to build without it though. It might be related to C++ name mangling.
* [ ] Implement a `WorkerService` for query planner that connects to a node running in `rpc` mode and sends message to workers using the libp2p transport. (For reference, [current implementation](https://github.com/subsquid/sql-archives/blob/main/src/main/java/io/subsquid/sql/exec/FlightWorkerService.java) using Apache Arrow Flight).
* [ ] Bug: The first message sent to a node is often lost somewhere. Also, sometimes connections between nodes break. To properly resolve this, we need to:
  * [ ] Wait for 'ack' response for every message,
  * [ ] Re-send message if an error occurs,
  * [ ] Re-connect if a connection to a node is lost.
* [ ] Bug: Local listen addresses, not reachable by other peers are propagated in Kademlia. Need to filter them out.
* [ ] Add 'autonat' protocol to the libp2p behaviour to detect if a node is behind NAT and reserve a relay slot only if necessary.
* [ ] Investigate why NAT hole punching (dcutr) fails, and nodes without public addresses always have to rely on relayed connections.
* [ ] Implement `CachingFilesProvider` for the worker. The interface is defined as follows:
    ```c++
    class CachingFilesProvider {
        void getCachedResources(std::vector<std::string> s3Uris, std::function<void(const Result<std::string>&)> readyCallback);
    };
    ```
