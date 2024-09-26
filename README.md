# SQD Network

This is a libp2p-based network that allows communication between nodes. See
[this page](https://github.com/subsquid/subsquid-network-contracts/wiki/Network-architecture)
for the network architecture.

## Transport crate

Contains the implementation of the libp2p transport protocol for the network.
It exposes the simple API for the following predefined actors on the network.

### Worker

Most nodes on the network are workers. Their main purpose is to process queries
from the Gateways.

### Scheduler

The Scheduler is a centralized controller of data distribution on the network.
It assigns chunks to workers based on collected pings and sends them the
assignments.

### Gateway

The Gateway is the entry point for the clients. It sends queries to the Workers
and receives the responses.

### Logs Collector

The Logs Collector saves the logs it receives to the persistent database for
further analysis.

### Pings Collector

The Pings Collector instances collect broadcasted pings from the Workers and
store them in the database.

## Messages crate

Contains protobuf schemas for the messages exchanged between nodes.

## Keygen

A simple binary for generating a keypair for a node.

The simplest way to use it is by using Docker. The following command will
generate a keypair and save it to the `key` file in the current directory, or
print the peer id if the file already exists.

```bash
docker run -u $(id -u):$(id -g) -v .:/host subsquid/keygen:tethys /host/key
```

## Bootnode

The simplest possible node which acts as a source of information about other
nodes. It is used by new nodes to connect to the network.

## Contract Client

A client for interacting with the smart contracts through the RPC node.
