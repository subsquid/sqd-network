FROM ubuntu:22.04 as builder

RUN apt update
RUN apt install -y -V build-essential cmake gdb clang-format \
        nano ca-certificates lsb-release wget curl git
RUN wget https://apache.jfrog.io/artifactory/arrow/$(lsb_release --id --short | tr 'A-Z' 'a-z')/apache-arrow-apt-source-latest-$(lsb_release --codename --short).deb
RUN apt install -y -V ./apache-arrow-apt-source-latest-$(lsb_release --codename --short).deb
RUN apt update
RUN apt install -y -V libarrow-dev
RUN apt install -y -V libarrow-flight-dev
RUN apt install -y -V libarrow-dataset-dev
RUN apt install -y -V openjdk-17-jdk-headless
RUN apt install -y -V protobuf-compiler
RUN apt install -y -V cmake
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.67.0 -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /usr/src
COPY . .

FROM builder as bootnode-builder

RUN cargo build --release --bin bootnode

FROM debian:bookworm-slim as bootnode
COPY --from=bootnode-builder /usr/src/target/release/bootnode /usr/local/bin/bootnode
CMD ["bootnode"]

FROM builder as worker-builder

RUN cargo build --release --features worker --bin node

FROM debian:bookworm-slim as worker
COPY --from=worker-builder /usr/src/target/release/node /usr/local/bin/node
CMD ["node"]

FROM builder as rpc-node-builder

RUN cargo build --release --features rpc --bin node

FROM debian:bookworm-slim as rpc-node
COPY --from=rpc-node-builder /usr/src/target/release/node /usr/local/bin/node
CMD ["node"]
