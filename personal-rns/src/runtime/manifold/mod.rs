//! The manifold: the engine-bolt above the passive engine.
//!
//! - [`core`] holds the substrate-neutral [`Manifold`] — it owns the engine and
//!   its registered worker and turns one drive cycle into intake → step →
//!   exhaust, touching no clock, RNG, sockets, or `.await`.
//! - [`snapshot`] holds the app-facing view ([`RuntimeSnapshot`]) the manifold
//!   surfaces each cycle — the one seam an app reads engine state through.
//! - [`impls`] holds the per-platform drivers that run a `Manifold` over time
//!   (drawing entropy, aggregating each worker's inbound, deciding when to
//!   cycle, and sleeping until the next deadline). Each is gated by its platform
//!   feature; the neutral core carries no gate.

mod core;
mod snapshot;
pub use core::Manifold;
pub use snapshot::{InterfaceView, RuntimeSnapshot};

#[cfg(any(feature = "embassy-host", feature = "std-host"))]
pub mod impls;
