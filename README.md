# Subsquid network

This is a libp2p-based network which allows to send arbitrary messages between nodes. It currently exposes only the
`bootnode` binary (a simple node which acts as a source of information about other nodes). Other nodes are using
`subsquid-network-transport` as a library.

## Running the bootnode

```
Usage: bootnode [OPTIONS]

Options:
  -k, --key <KEY>
          Path to libp2p key file [env: KEY_PATH=]
      --p2p-listen-addrs <P2P_LISTEN_ADDRS>...
          Addresses on which the p2p node will listen [env: P2P_LISTEN_ADDRS=] [default: /ip4/0.0.0.0/udp/0/quic-v1]
      --p2p-public-addrs <P2P_PUBLIC_ADDRS>...
          Public address(es) on which the p2p node can be reached [env: P2P_PUBLIC_ADDRS=]
      --boot-nodes <BOOT_NODES>...
          Connect to boot node '<peer_id> <address>'. [env: BOOT_NODES=]
      --bootstrap
          Bootstrap kademlia. Makes node discoverable by others. [env: BOOTSTRAP=]
  -h, --help
          Print help
  -V, --version
          Print version
```

## Docker image

There is a Dockerfile provided which allows to build bootnode image:

```
docker build ./transport --target bootnode
```
