//! The runtime layer — the engine-bolt above the passive engine.
//!
//! [`Runtime`] owns the engine and its registered workers as siblings and turns
//! one drive cycle into intake → step → exhaust. It is substrate-neutral (no
//! clock, RNG, sockets, or `.await`): the single generic [`run`] loop asks a
//! [`Host`] for one cycle's clock + entropy + inbound at a time, so a platform
//! varies only in its `Host` impl — drawing entropy, aggregating each worker's
//! inbound, and sleeping until the next deadline.
//!
//! - `core` holds the [`Runtime`] + the [`run`] loop.
//! - [`host`] holds the substrate seam ([`Host`]) + its per-platform impls.
//! - [`channels`] holds the per-platform mailbox + snapshot-channel types a host
//!   wires its workers and apps through.
//! - `snapshot` holds the app-facing [`RuntimeSnapshot`] view.

pub mod channels;
mod contract;
mod core;
pub mod host;
mod snapshot;

pub use contract::{run_contract, ContractRuntime, ContractStepOutput};
pub use core::{run, Runtime};
pub use host::{block_on, CycleStamp, Host};
pub use snapshot::{InterfaceView, RuntimeSnapshot};
