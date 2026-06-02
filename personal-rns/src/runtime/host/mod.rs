//! The runtime host seam — the substrate contract a platform implements so the
//! single generic [`run`](crate::runtime::run) loop can drive a
//! [`Runtime`](super::Runtime), plus the [`block_on`] shim for executor-less
//! hosts.
//!
//! - `core` holds the substrate-neutral contract: the [`Host`] trait,
//!   [`CycleStamp`], and [`block_on`].
//! - [`impls`] holds the per-platform host catalogue (`LinuxSync`,
//!   `EmbassyHost`, …), each gated by its platform feature; the neutral contract
//!   carries no gate.
//!
//! A [`Runtime`](super::Runtime) is substrate-neutral:
//! [`cycle_once`](super::Runtime::cycle_once) turns one cycle's inbound + clock +
//! entropy into intake → tick → exhaust but never reads a clock, draws
//! randomness, or sleeps. A [`Host`] is the other half — it owns the clock, the
//! CSPRNG, the inbound mailbox, and the sleep. Sync vs async resolves in one
//! direction: the loop is `async` because an executor-backed host genuinely
//! suspends while sleeping, whereas a sync poll-loop host blocks inside
//! [`Host::wait`] and never yields — so its futures are always ready and
//! [`block_on`] drives the same `run` future with no executor. You can always
//! present sync work as a ready future; you cannot present async work as a
//! blocking call.

mod core;
pub use core::{block_on, CycleStamp, Host};

#[cfg(any(
    feature = "embassy-contract",
    feature = "embassy-host",
    feature = "std-host"
))]
pub mod impls;
