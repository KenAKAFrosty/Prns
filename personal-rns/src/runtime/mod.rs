//! The runtime layer — the engine-bolt above the passive engine.
//!
//! [`ContractRuntime`] owns the engine and its started interfaces as siblings and
//! turns one drive cycle into intake → step → exhaust. It is substrate-neutral (no
//! clock, RNG, sockets, or `.await`): the single generic [`run_contract`] loop asks a
//! [`Host`] for one cycle's clock + entropy at a time and pools each interface's seam,
//! so a platform varies only in its `Host` impl + the seam its interfaces meet it on.
//!
//! - `contract` holds the [`ContractRuntime`] + the [`run_contract`] loop.
//! - `core` holds the shared per-interface traffic meter.
//! - [`host`] holds the substrate seam ([`Host`]) + its per-platform impls.
//! - [`channels`] holds the per-platform seam + snapshot-channel types a host wires
//!   its interfaces and apps through.
//! - `snapshot` holds the app-facing [`RuntimeSnapshot`] view.

pub mod channels;
mod contract;
mod core;
pub mod host;
mod snapshot;

pub use contract::{run_contract, ContractRuntime, ContractStepOutput};
pub use host::{block_on, CycleStamp, Host};
pub use snapshot::{InterfaceView, RuntimeSnapshot};
