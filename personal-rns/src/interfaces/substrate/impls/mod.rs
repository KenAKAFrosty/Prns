#[cfg(feature = "std-host")]
mod std;
#[cfg(feature = "std-host")]
pub use std::*;

#[cfg(feature = "embassy-seam")]
mod embassy;
#[cfg(feature = "embassy-seam")]
pub use embassy::*;
