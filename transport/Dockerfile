FROM rust:1.70-bookworm as builder

RUN apt update
RUN apt install -y -V protobuf-compiler

WORKDIR /usr/src
COPY . .

FROM builder as bootnode-builder

RUN cargo build --release --bin bootnode

FROM debian:bookworm-slim as bootnode
COPY --from=bootnode-builder /usr/src/target/release/bootnode /usr/local/bin/bootnode
CMD ["bootnode"]

FROM builder as rpc-node-builder

RUN cargo build --release --features rpc --bin node

FROM debian:bookworm-slim as rpc-node
COPY --from=rpc-node-builder /usr/src/target/release/node /usr/local/bin/node
CMD ["node"]
