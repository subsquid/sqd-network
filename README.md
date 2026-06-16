# SQD Network

Rust implementation of the peer-to-peer layer of SQD Network, the decentralized
data lake behind [SQD](https://sqd.dev). Nodes communicate over
[libp2p](https://github.com/libp2p/rust-libp2p), exchanging queries, query
results, heartbeats, and logs. For the network design, see the
[network architecture wiki](https://github.com/subsquid/subsquid-network-contracts/wiki/Network-architecture)
and the [SQD Network docs](https://docs.sqd.dev/en/network).

## How it fits into SQD

SQD Network stores blockchain data as chunks distributed across worker nodes.
A scheduler assigns chunks to workers; portals route client queries to the
workers that hold the relevant data and collect the results. SQD Portal, the
data API over 200+ chains, is built on top of this network. This repository
provides the transport protocol, message formats, and a few supporting node
binaries used by those actors.

## Node roles

The transport layer exposes APIs for the predefined actors on the network:

- **Worker**: holds data chunks and answers queries routed to it.
- **Scheduler**: assigns chunks to workers based on collected heartbeats and
  distributes the assignments.
- **Portal**: entry point for clients. It sends queries to workers and
  collects responses. (Previously called Gateway.)
- **Pings collector**: collects worker heartbeats and stores them.
- **Logs collector**: persists query logs received from workers for analysis.

## Workspace layout

This is a Cargo workspace (`crates/*`).

| Crate | Type | Description |
|---|---|---|
| `sqd-network-transport` | library | libp2p transport and per-actor behaviours. Actors are gated behind Cargo features (`worker`, `portal`, `observer`, `pings-collector`, `logs-collector`, `portal-logs-collector`, `sql-client`). |
| `sqd-messages` | library | Protobuf (prost) message schemas for inter-node communication. Source schema: `crates/messages/proto/messages.proto`. |
| `sqd-assignments` | library | FlatBuffers chunk-assignment format with `builder` and `reader` features. Schema: `crates/assignments/schema/assignment.fbs`. |
| `sqd-contract-client` | library | Reads onchain registries, allocations, and node sets from the network's smart contracts over an RPC node ([ethers](https://github.com/gakonst/ethers-rs)). |
| `sqd-bootnode` | binary | Minimal node that helps new nodes discover peers and bootstrap into the network. |
| `sqd-keygen` | binary | Generates a libp2p keypair for a node and prints its peer ID. |
| `sqd-observer` | binary | Joins the network, observes gossip and peers, and exposes Prometheus metrics over HTTP. |

## Build

Requires the Rust toolchain pinned in `rust-toolchain` (1.89.0) and
`protoc` (protobuf-compiler) for building `sqd-messages`.

```bash
cargo build --release
```

Build a specific binary:

```bash
cargo build --release --bin sqd-bootnode
cargo build --release --bin sqd-keygen
cargo build --release --bin sqd-observer
```

The transport library compiles only the actor APIs you enable. For example, to
build with the worker actor:

```bash
cargo build -p sqd-network-transport --features worker
```

## Docker

The `Dockerfile` builds the three binaries and provides image targets for
`bootnode`, `keygen`, and `observer`. The published images are
`subsquid/bootnode`, `subsquid/keygen`, and `subsquid/observer`.

## Run

### Keygen

Generate a keypair and write it to a `key` file in the current directory, or
print the peer ID if the file already exists:

```bash
docker run -u $(id -u):$(id -g) -v .:/host subsquid/keygen:latest /host/key
```

### Bootnode and observer

Both nodes share the transport CLI flags (also settable via environment
variables):

| Flag | Env | Description |
|---|---|---|
| `--key` | `KEY_PATH` | Path to the libp2p key file. |
| `--p2p-listen-addrs` | `P2P_LISTEN_ADDRS` | Multiaddrs the node listens on. |
| `--p2p-public-addrs` | `P2P_PUBLIC_ADDRS` | Public multiaddrs the node advertises. |
| `--boot-nodes` | `BOOT_NODES` | Boot nodes as `<peer_id> <address>` entries. |
| `--rpc-url` | `RPC_URL` | RPC node URL for reading network contracts. |

The observer additionally takes `--port` (HTTP port for metrics, default 8000)
and `--network` (`mainnet` or `tethys`).

## Documentation

- SQD Network: https://docs.sqd.dev/en/network
- SQD: https://sqd.dev

## License

AGPL-3.0-or-later. See [LICENSE.md](LICENSE.md).
