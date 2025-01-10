# P2P transport debugging tool

## Usage

Run `cargo run --example=debug -- --help` to see the available options.

Any combination of the supported protocols may be run.
For example, you may run Kademlia discovery and ping all connected peers by combining the `--ping` and `--kad` options.

## Examples:

### Ping some other peer

```sh
KEY_PATH=dbg.key cargo run --example=debug -- --ping -d /dns4/mainnet.subsquid.io/udp/22445/quic-v1
```

### Send identify request

Asks for the protocols the peer supports and our observed address

```sh
KEY_PATH=dbg.key cargo run --example=debug -- --identify -d /dns4/mainnet.subsquid.io/udp/22445/quic-v1
```

### Run AutoNAT check

Asks the peer to dial us back to confirm if any of the external addresses are reachable.
A peer is dialed explicitly to get the observed address via the identify protocol.

```sh
KEY_PATH=dbg.key cargo run --example=debug -- --autonat -d /dns4/mainnet.subsquid.io/udp/22445/quic-v1
```

### Run Kademlia bootstrap

Discovers existing peer addresses from the DHT.

```sh
KEY_PATH=dbg.key cargo run --example=debug -- --kad --network=tethys
```

### Listen for gossipsub messages

```sh
KEY_PATH=dbg.key cargo run --example=debug -- --gossipsub --network=tethys
```

