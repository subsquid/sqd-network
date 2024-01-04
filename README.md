# Subsquid network

This is a libp2p-based network which allows to send arbitrary messages between nodes.

## Binaries
1. `bootnode` – A simple node which acts as relay server and a source of information about other nodes.
2. `node` – The proper node which communicates exposer RPC to communicate with the network (see below).

## Running the node

```
Usage: node [OPTIONS]

Options:
  -k, --key <KEY>
          Path to libp2p key file [env: KEY_PATH=]
      --p2p-listen-addr <P2P_LISTEN_ADDR>
          Address on which the p2p node will listen [env: P2P_LISTEN_ADDR=] [default: /ip4/0.0.0.0/tcp/0]
      --p2p-public-addrs <P2P_PUBLIC_ADDRS>...
          Public address(es) on which the p2p node can be reached [env: P2P_PUBLIC_ADDRS=]
      --boot-nodes <BOOT_NODES>...
          Connect to boot node '<peer_id> <address>'. [env: BOOT_NODES=]
      --bootstrap
          Bootstrap kademlia. Makes node discoverable by others. [env: BOOTSTRAP=]
  -r, --relay [<RELAY>]
          Connect to relay. If address not specified, one of the boot nodes is used. [env: RELAY=]
      --rpc-listen-addr <RPC_LISTEN_ADDR>
          Listen address for the rpc server [env: RPC_LISTEN_ADDR=] [default: 0.0.0.0:50051]
  -h, --help
          Print help
  -V, --version
          Print version

```

The node starts a gRPC server allowing to send and receive messages to/from the network by another process. The server API is described below.
```protobuf
syntax = "proto3";
package p2p_transport;

service P2PTransport {
  rpc LocalPeerId(Empty) returns (PeerId);
  rpc GetMessages(Empty) returns (stream Message);
  rpc SendMessage(Message) returns (Empty);
  rpc ToggleSubscription(Subscription) returns (Empty);
  rpc Sign(Bytes) returns (Bytes);
  rpc VerifySignature(SignedData) returns (VerificationResult);
}

message Empty {}

message PeerId {
  string peer_id = 1;
}

message Message {
  // None for outgoing broadcast messages, Some for others
  optional string peer_id = 1;
  // None for direct messages, Some for broadcast messages
  optional string topic = 2;
  bytes content = 3;
}

message Subscription {
  string topic = 1;
  bool subscribed = 2;
  bool allow_unordered = 3;
}

message Bytes {
  bytes bytes = 1;
}

message SignedData {
  bytes data = 1;
  bytes signature = 2;
  string peer_id = 3;
}

message VerificationResult {
  bool signature_ok = 1;
}

```

## Docker images
There is a Dockerfile provided which allows to build 2 images:
1. `docker build ./transport --target rpc-node` – rpc node
2. `docker build ./transport --target bootnode` – bootnode
