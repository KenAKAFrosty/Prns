#[cfg(feature = "std-host")]
mod std;
#[cfg(feature = "usb-auto")]
pub use std::usb_auto_interface;

#[cfg(feature = "embassy-contract")]
mod embassy;
#[cfg(feature = "embassy-contract")]
pub use embassy::serve;
