#[cfg(feature = "std-host")]
mod std;
#[cfg(feature = "usb-auto")]
pub use std::usb_auto_interface;

//REVIEW i'm also not sure why we have like 3 different embasy terms. isn't there an emebassy-host and another one as well?

#[cfg(feature = "embassy-contract")]
mod embassy;
#[cfg(feature = "embassy-contract")]
pub use embassy::serve;
