FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:0.1.62-rust-1.74-slim-bookworm AS chef
WORKDIR /app

FROM --platform=$BUILDPLATFORM chef AS planner

COPY build.rs .
COPY Cargo.toml .
COPY proto ./proto
COPY src ./src

RUN cargo chef prepare --recipe-path recipe.json

FROM --platform=$BUILDPLATFORM chef as builder

RUN apt update
RUN apt install -y -V protobuf-compiler

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY build.rs .
COPY Cargo.toml .
COPY proto ./proto
COPY src ./src

FROM builder as bootnode-builder

RUN cargo build --release --bin bootnode

FROM --platform=$BUILDPLATFORM debian:bookworm-slim as bootnode
COPY --from=bootnode-builder /app/target/release/bootnode /usr/local/bin/bootnode
CMD ["bootnode"]

ENV P2P_LISTEN_ADDR="/ip4/0.0.0.0/tcp/12345"

RUN apt update && apt install net-tools
RUN echo "PORT=\${P2P_LISTEN_ADDR##*/}; netstat -an | grep \$PORT > /dev/null; if [ 0 != \$? ]; then exit 1; fi;" > ./healthcheck.sh
RUN chmod +x ./healthcheck.sh
HEALTHCHECK --interval=5s CMD ./healthcheck.sh

FROM builder as rpc-node-builder

RUN cargo build --release --bin node
RUN cargo build --release --bin keygen

FROM --platform=$BUILDPLATFORM debian:bookworm-slim as rpc-node
COPY --from=rpc-node-builder /app/target/release/node /usr/local/bin/node
COPY --from=rpc-node-builder /app/target/release/keygen /usr/local/bin/keygen
CMD ["node"]

ENV P2P_LISTEN_ADDRS="/ip4/0.0.0.0/tcp/12345"
ENV RPC_LISTEN_ADDR="0.0.0.0:50051"
ENV BOOTSTRAP="true"

RUN apt update && apt install net-tools
RUN echo "PORT=\${RPC_LISTEN_ADDR##*:}; netstat -an | grep \$PORT > /dev/null; if [ 0 != \$? ]; then exit 1; fi;" > ./healthcheck.sh
RUN chmod +x ./healthcheck.sh
HEALTHCHECK --interval=5s CMD ./healthcheck.sh
