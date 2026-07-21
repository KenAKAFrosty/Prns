#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
mod defaults;
mod registration;

#[cfg(any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto"))]
pub use defaults::DefaultAutoInterfaces;
