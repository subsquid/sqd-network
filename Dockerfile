# https://www.lpalmieri.com/posts/fast-rust-docker-builds/#cargo-chef
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:0.1.67-rust-1.80.1-slim-bookworm AS chef
WORKDIR /app


FROM --platform=$BUILDPLATFORM chef AS planner

COPY Cargo.toml .
COPY Cargo.lock .

COPY crates ./crates

RUN cargo chef prepare --recipe-path recipe.json


FROM --platform=$BUILDPLATFORM chef AS builder

RUN --mount=target=/var/lib/apt/lists,type=cache,sharing=locked \
    --mount=target=/var/cache/apt,type=cache,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update \
    && apt-get -y install build-essential protobuf-compiler

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml .
COPY Cargo.lock .
COPY crates ./crates

RUN cargo build --release --bin sqd-bootnode --bin sqd-keygen --bin sqd-observer


FROM --platform=$BUILDPLATFORM debian:bookworm-slim AS base

RUN --mount=target=/var/lib/apt/lists,type=cache,sharing=locked \
    --mount=target=/var/cache/apt,type=cache,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update \
    && apt-get -y install ca-certificates net-tools


FROM --platform=$BUILDPLATFORM base AS bootnode
COPY --from=builder /app/target/release/sqd-bootnode /usr/local/bin/sqd-bootnode
COPY --from=builder /app/target/release/sqd-keygen /usr/local/bin/keygen
CMD ["sqd-bootnode"]

ENV P2P_LISTEN_ADDRS="/ip4/0.0.0.0/udp/12345/quic-v1"

COPY p2p_healthcheck.sh ./healthcheck.sh
RUN chmod +x ./healthcheck.sh
HEALTHCHECK --interval=5s CMD ./healthcheck.sh


FROM --platform=$BUILDPLATFORM base AS observer
COPY --from=builder /app/target/release/sqd-observer /usr/local/bin/sqd-observer
CMD ["sqd-observer"]

ENV P2P_LISTEN_ADDRS="/ip4/0.0.0.0/udp/12345/quic-v1"


FROM --platform=$BUILDPLATFORM base AS keygen
COPY --from=builder /app/target/release/sqd-keygen /usr/local/bin/keygen
ENTRYPOINT ["keygen"]