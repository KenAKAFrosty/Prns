//! The runtime layer — the engine-bolt above the passive engine.
//!
//! [`Runtime`] owns the engine and its registered workers as siblings and turns
//! one drive cycle into intake → step → exhaust. It is substrate-neutral (no
//! clock, RNG, sockets, or `.await`): the single generic [`run`] loop asks a
//! [`Host`] for one cycle's clock + entropy + inbound at a time, so a platform
//! varies only in its `Host` impl — drawing entropy, aggregating each worker's
//! inbound, and sleeping until the next deadline.

pub mod host;
pub mod manifold;

pub use host::{block_on, CycleStamp, Host};
pub use manifold::{run, InterfaceView, Runtime, RuntimeSnapshot};
