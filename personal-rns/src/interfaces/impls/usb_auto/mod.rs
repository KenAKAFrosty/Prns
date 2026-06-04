pub mod core;
#[cfg(feature = "std-host")]
mod discovery;
mod impls;
#[cfg(feature = "embassy-contract")]
pub use impls::serve;
#[cfg(feature = "usb-auto")]
pub use impls::usb_auto_interface;
