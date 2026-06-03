//! Concrete per-platform seams — one file per substrate, mirroring the storage
//! recipe. Each backs the [`Substrate`](super::Substrate) contract with a
//! platform's lane storage: std with allocated `rtrb` rings, embassy with `'static`
//! `Channel`s.

#[cfg(feature = "std-host")]
mod std;
#[cfg(feature = "std-host")]
pub use std::*;

#[cfg(feature = "embassy-seam")]
mod embassy;
#[cfg(feature = "embassy-seam")]
pub use embassy::*;
