//! Per-platform inbound-mailbox + snapshot-channel types a [`Host`](super::Host)
//! wires its workers through.
//!
//! Each module is the substrate-specific plumbing between the workers and the
//! neutral [`Runtime`](super::Runtime): the shared inbound mailbox a worker
//! stamps into (the host drains it each cycle) and, on embassy, the `Watch` the
//! runtime fires its snapshot out on. One module per platform class, gated by its
//! feature: `embassy` (async tasks, no_std) and [`std_host`] (threads + std
//! channels, the host twin).

#[cfg(feature = "embassy-host")]
pub mod embassy;

#[cfg(feature = "std-host")]
pub mod std_host;
