//! A vendored copy of the `libp2p-stream` crate, carrying two local patches.
//!
//! Upstream source: <https://github.com/libp2p/rust-libp2p>, `protocols/stream` at commit
//! `aefbfbdcef203f9d48af864335fe737b4445a042` (`libp2p-stream` v0.4.0-alpha). MIT licensed —
//! see the `LICENSE` file next to this module.
//!
//! # Local patches
//!
//! Both fix streams being silently dropped under concurrency, and neither is reachable from
//! outside the crate, hence the copy:
//!
//! * The dial request channel in [`Behaviour::new`] is unbounded. Upstream uses
//!   `mpsc::channel(0)` and discards the `try_send` error in `Shared::sender`, so only one dial
//!   request fits between two swarm polls: a concurrent `open_stream` for a second, not yet
//!   connected peer is dropped and its caller waits for a dial that never happens.
//! * [`Control::accept_with_capacity`] is added. Upstream's [`Control::accept`] registers an
//!   `mpsc::channel(0)`, so an inbound stream is dropped unless [`IncomingStreams`] happens to
//!   be polled at that moment, losing bursts of concurrent requests. `accept` keeps the
//!   unbuffered behaviour.
//!
//! Apart from those two changes, the module paths (`crate::` -> `super::`), `rand::thread_rng()`
//! -> `rand::rng()` (renamed in rand 0.9) and this repository's rustfmt settings, the code is
//! upstream's. Keep it that way: refreshing this copy should stay a matter of taking the newer
//! upstream files, re-applying the two patches and running `cargo fmt`.

// Vendored code is exempt from this workspace's lints: it should stay as close to upstream as
// possible rather than follow local style. Applies to the child modules as well.
#![allow(deprecated, clippy::all, clippy::pedantic, clippy::nursery)]

mod behaviour;
mod control;
mod handler;
mod shared;
#[cfg(test)]
mod tests;
mod upgrade;

pub use behaviour::{AlreadyRegistered, Behaviour};
pub use control::{Control, IncomingStreams, OpenStreamError};
