FROM rust:1.70-bookworm as builder

RUN apt update
RUN apt install -y -V protobuf-compiler

WORKDIR /usr/src
COPY build.rs .
COPY Cargo.toml .
COPY proto ./proto
COPY src ./src

FROM builder as bootnode-builder

RUN cargo build --release --bin bootnode

FROM debian:bookworm-slim as bootnode
COPY --from=bootnode-builder /usr/src/target/release/bootnode /usr/local/bin/bootnode
CMD ["bootnode"]

ENV LISTEN_ADDR="/ip4/0.0.0.0/tcp/12345"
RUN apt update && apt install net-tools
RUN echo "PORT=\${LISTEN_ADDR##*/}; netstat -an | grep \$PORT > /dev/null; if [ 0 != \$? ]; then exit 1; fi;" > ./healthcheck.sh
RUN chmod +x ./healthcheck.sh
HEALTHCHECK --interval=5s CMD ./healthcheck.sh

FROM builder as rpc-node-builder

RUN cargo build --release --features rpc --bin node

FROM debian:bookworm-slim as rpc-node
COPY --from=rpc-node-builder /usr/src/target/release/node /usr/local/bin/node
CMD ["node"]

ENV P2P_LISTEN_ADDR="/ip4/0.0.0.0/tcp/12345"
ENV RPC_LISTEN_ADDR="0.0.0.0:50051"
ENV BOOTSTRAP="true"

RUN apt update && apt install net-tools
RUN echo "PORT=\${RPC_LISTEN_ADDR##*:}; netstat -an | grep \$PORT > /dev/null; if [ 0 != \$? ]; then exit 1; fi;" > ./healthcheck.sh
RUN chmod +x ./healthcheck.sh
HEALTHCHECK --interval=5s CMD ./healthcheck.sh
