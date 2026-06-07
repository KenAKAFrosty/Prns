#[cfg(feature = "usb-auto")]
mod std;
#[cfg(feature = "usb-auto")]
pub use std::usb_auto_interface;

#[cfg(feature = "usb-auto")]
mod android;
#[cfg(feature = "usb-auto")]
pub use android::{android_usb_auto_interface, AndroidUsbBridge};

#[cfg(feature = "embassy-contract")]
mod embassy;
#[cfg(feature = "embassy-contract")]
pub use embassy::serve;
