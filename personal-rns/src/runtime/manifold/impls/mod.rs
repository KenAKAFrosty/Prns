//! Per-platform drivers that run a [`Manifold`](super::Manifold) over time.
//!
//! Each driver is the substrate-specific half of the runtime: it owns the
//! clock, the sleep primitive, and the host's entropy source, and drives the
//! neutral `Manifold` through them. One module per platform class, gated by its
//! feature: `embassy` (async tasks, no_std) and [`std_host`] (threads + std
//! channels, the host twin).

#[cfg(feature = "embassy-host")]
pub mod embassy;

#[cfg(feature = "std-host")]
pub mod std_host;
