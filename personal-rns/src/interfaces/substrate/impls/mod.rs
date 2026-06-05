#[cfg(feature = "std-sync-host")]
mod std;
#[cfg(feature = "std-sync-host")]
pub use std::*;

#[cfg(feature = "embassy-seam")]
mod embassy;
#[cfg(feature = "embassy-seam")]
pub use embassy::*;
