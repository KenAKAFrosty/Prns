pub mod core;
#[cfg(feature = "std-sync-host")]
mod discovery;
mod impls;
#[cfg(feature = "embassy-contract")]
pub use impls::serve;
#[cfg(feature = "usb-auto")]
pub use impls::{android_usb_auto_interface, usb_auto_interface, AndroidUsbBridge};
