//! Per-platform drivers that run a [`Manifold`](super::Manifold) over time.
//!
//! Each driver is the substrate-specific half of the runtime: it owns the
//! clock, the sleep primitive, and the host's entropy source, and drives the
//! neutral `Manifold` through them. One module per platform class, gated by its
//! feature.

pub mod embassy;
