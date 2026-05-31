//! The manifold: the engine-bolt above the passive engine.
//!
//! - [`core`] holds the substrate-neutral [`Manifold`] — it owns the engine and
//!   its registered worker and turns one drive cycle into intake → step →
//!   exhaust, touching no clock, RNG, sockets, or `.await`.
//! - [`impls`] holds the per-platform drivers that run a `Manifold` over time
//!   (drawing entropy, aggregating each worker's inbound, deciding when to
//!   cycle, and sleeping until the next deadline). Each is gated by its platform
//!   feature; the neutral core carries no gate.

mod core;
pub use core::Manifold;

#[cfg(feature = "embassy-host")]
pub mod impls;
