FROM rust:1.67-bookworm as builder

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
RUN rustup toolchain install nightly-2023-02-01

WORKDIR /usr/src/myapp
COPY . .
RUN cargo +nightly-2023-02-01 install --path .

FROM debian:bookworm-slim
COPY --from=builder /usr/local/cargo/bin/bootnode /usr/local/bin/bootnode
CMD ["bootnode"]
