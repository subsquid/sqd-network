# subsquid-network

A network of decentralized archives and workers.

## Archives (Data Source)

`./crates/pallet-data-source`

Archive is a service that ingests raw on-chain data, stores it into persistent storage in a normalized way and exposes it via API for downstream data pipelines (such as Squid Processor) and ad-hoc exploration. Compared to data access using a conventional chain node RPC, an archive allows one to access data in a more granular fashion and from multiple blocks at once, thanks to its rich batching and filtering capabilities.

## Workers

`./crate/pallet-workers`

Worker is a part of the network that provides resources for executing requests submitted by the network. To execute the request, the worker should run docker related command that is part of the request.

### Requirements

- Worker should pass some registration to be a part of the network.
- Worker should stake required SQD tokens.
- Worker should listen the network for events to execute task.
- Worker should use approved (by the network) docker images.
- There is a range of workers number when it's profitable to run it.

## Workers scheduler

`./crates/pallet-workers-scheduler`

Workers schedule is a part of the network that is responsible for assigning a specific task to the worker for incoming requests from clients. It should choose available suitable worker based on some predefined algorithm. The task is assigned by submitting event.

## Vesting

`./crate/pallet-vesting`

A simple module providing a means of placing a linear curve on an account's locked balance (via Substrate `LockableCurrency` trait implementation). It should be possible to create vesting for any account by any account, removing vesting for core team in some cases by sudo call, unlock tokens.

## Rust

Install rust related tools by running the command `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.

## Run

You can run the network as usual substrate-based network. Run `cargo run -- --dev --tmp` for running the node in dev mode with temporary data.

## Test

There is an implemented unittest example for `pallet-vesting` for demo purposes. Use `cargo test` to run unittests for network.
