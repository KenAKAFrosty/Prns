//! Concrete serial workers — one file per substrate. Each owns its platform's byte
//! transport and runs the shared read→deframe→stamp / drain→frame→write loop.

#[cfg(feature = "std-host")]
mod std;
#[cfg(feature = "std-host")]
pub use std::std_serial_interface;

#[cfg(feature = "embassy-contract")]
mod embassy;
#[cfg(feature = "embassy-contract")]
pub use embassy::serve;
