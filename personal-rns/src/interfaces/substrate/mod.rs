//! The substrate: the seam an interface meets the runtime through, and the
//! per-platform backings that carry it.
//!
//! [`core`] is the substrate-neutral contract — the [`Substrate`] trait and the
//! [`InterfaceWorkerContext`] / lane traits an interface is written against. The
//! concrete per-platform seams (std rings, embassy channels) live under `impls/`,
//! mirroring the storage recipe.

pub mod core;
pub use core::*;
