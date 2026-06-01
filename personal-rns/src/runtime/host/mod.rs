//! The runtime host seam — the contract a platform implements to drive a
//! [`Manifold`](super::Manifold), the single generic [`run`] loop over it, and
//! the [`block_on`] shim for executor-less hosts.
//!
//! - `core` holds the substrate-neutral contract: the [`RuntimeHost`] trait,
//!   [`CycleStamp`], the generic [`run`] loop, and [`block_on`].
//! - [`impls`] holds the per-platform host catalogue (`LinuxSync`,
//!   `Esp32S3Embassy`, `Esp32C6Sync`, …), each gated by its platform feature;
//!   the neutral contract carries no gate.
//!
//! [`Manifold`](super::Manifold) is substrate-neutral:
//! [`cycle_once`](super::Manifold::cycle_once) turns one cycle's fuel into
//! intake → tick → exhaust but never reads a clock, draws randomness, or sleeps.
//! A [`RuntimeHost`] is the other half — it owns the clock, the CSPRNG, the
//! inbound mailbox, and the sleep. Sync vs async resolves in one direction: the
//! loop is `async` because an executor-backed host genuinely suspends while
//! sleeping, whereas a sync poll-loop host blocks inside [`RuntimeHost::wait`]
//! and never yields — so its futures are always ready and [`block_on`] drives
//! the same `run` future with no executor. You can always present sync work as a
//! ready future; you cannot present async work as a blocking call.

mod core;
pub use core::{block_on, run, CycleStamp, RuntimeHost};

#[cfg(any(feature = "embassy-host", feature = "std-host"))]
pub mod impls;
