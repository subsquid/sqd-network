# gRPC over libp2p

This is a libp2p-based network which allows to send gRPC requests between nodes.

To start a node run the following command.
```shell
cargo run --bin node
```
In order to listen for incoming connections, add the `--listen` flag.
Either this or `--dial` should always be provided (could use both).
```shell
cargo run --bin node -- --listen /ip4/127.0.0.1/tcp/12345
```
In order to connect to another listening node, add the `--dial` flag.
This flag could be repeated, which will open multiple connections.
Either this or `--listen` should always be provided (could use both).
```shell
cargo run --bin node -- --dial /ip4/127.0.0.1/tcp/12345
```
In order for the node to be discoverable, add the `--bootstrap` flag.
Otherwise, it will be able to communicate only with nodes which directly connect to it.
Only bootnodes with fixed, publicly known addresses should omit this flag.
```shell
cargo run --bin node -- --dial /ip4/127.0.0.1/tcp/12345 --bootstrap
```
In order to start sending periodic messages to another node, add the `--send-messages` flag
and provide the peer ID as argument (*not* the address).
Messages should appear in the console of the recipient worker.

**Node 1:**
```shell
cargo run --bin node -- --listen
2023-03-02T15:12:37.086Z INFO  [grpc_libp2p::transport] Local peer ID: 12D3KooWCFxr6LjpEneszGaogeoxJM3pHMrob4VTajWbMFmxm7Zc
2023-03-02T15:12:37.091Z INFO  [grpc_libp2p::transport] Listening on /ip4/127.0.0.1/tcp/12345
[2023-03-02 16:12:37.093] [info] Configure worker config =
[2023-03-02 16:12:37.093] [info] Start worker
[2023-03-02 16:12:48.620] [info] Message received from 12D3KooWAscbPWA7VQPGoudjS8Wrrjdr4pqzGo1AHnUjT5k7KiBo: Hello!
[2023-03-02 16:12:53.105] [info] Message received from 12D3KooWAscbPWA7VQPGoudjS8Wrrjdr4pqzGo1AHnUjT5k7KiBo: Hello!
...
```

**Node 2:**
```shell
cargo run --bin node -- --dial /ip4/127.0.0.1/tcp/12345 --bootstrap --send-messages 12D3KooWCFxr6LjpEneszGaogeoxJM3pHMrob4VTajWbMFmxm7Zc
2023-03-02T15:12:48.099Z INFO  [grpc_libp2p::transport] Local peer ID: 12D3KooWAscbPWA7VQPGoudjS8Wrrjdr4pqzGo1AHnUjT5k7KiBo
2023-03-02T15:12:48.102Z INFO  [grpc_libp2p::transport] Dialing /ip4/127.0.0.1/tcp/12345
2023-03-02T15:12:48.102Z INFO  [grpc_libp2p::transport] Bootstrapping kademlia
[2023-03-02 16:12:48.103] [info] Configure worker config = 12D3KooWCFxr6LjpEneszGaogeoxJM3pHMrob4VTajWbMFmxm7Zc
[2023-03-02 16:12:48.103] [info] Start worker
[2023-03-02 16:12:48.103] [info] Start sending messages
[2023-03-02 16:12:48.103] [info] Sending message to worker 12D3KooWCFxr6LjpEneszGaogeoxJM3pHMrob4VTajWbMFmxm7Zc
[2023-03-02 16:12:53.104] [info] Sending message to worker 12D3KooWCFxr6LjpEneszGaogeoxJM3pHMrob4VTajWbMFmxm7Zc
...
```
