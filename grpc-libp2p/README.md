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
One a node is running, you can use it to send a 'hello world' request by sending
`<peer_id> <custom_message>` to the process' stdin. If the request was successful,
a reply should be displayed.
```shell
cargo run --bin node -- --dial /ip4/127.0.0.1/tcp/12345 --bootstrap
2023-02-20T11:34:44.634Z INFO  [grpc_libp2p::transport] Local peer ID: 12D3KooWBYpBnbQBJC5mtP674YYDYPcXUcTULZcqj3Fn5VGtiisx
2023-02-20T11:34:44.636Z INFO  [grpc_libp2p::transport] Dialing /ip4/127.0.0.1/tcp/12345
2023-02-20T11:34:44.636Z INFO  [grpc_libp2p::transport] Bootstrapping kademlia
12D3KooWCqcSvVS1StJzggyFzs6dGeqcSxxLydB9toYSnPxQW6VJ subsquid
2023-02-20T11:35:03.626Z INFO  [node] Received response: Hello subsquid!
```
