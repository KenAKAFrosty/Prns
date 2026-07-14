#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(any(feature = "esp-now", feature = "ble-trouble"))]
extern crate alloc;

#[cfg(feature = "log")]
#[allow(unused_imports)]
pub(crate) mod diagnostic_log {
    pub(crate) use log::{debug, error, info, trace, warn};
}

#[cfg(not(feature = "log"))]
#[allow(unused_imports, unused_macros)]
pub(crate) mod diagnostic_log {
    macro_rules! disabled {
        ($($arg:tt)*) => {{
            if false {
                let _ = format_args!($($arg)*);
            }
        }};
    }

    pub(crate) use disabled as debug;
    pub(crate) use disabled as error;
    pub(crate) use disabled as info;
    pub(crate) use disabled as trace;
    pub(crate) use disabled as warn;
}

#[cfg(feature = "tcp")]
pub mod tcp;

#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(feature = "lora")]
pub mod lora;

#[cfg(feature = "esp-now")]
pub mod esp_now;

#[cfg(feature = "ble")]
pub mod ble;

#[cfg(feature = "ble-trouble")]
pub mod ble_trouble;

#[cfg(feature = "usb")]
pub mod usb;

#[cfg(feature = "usb-device")]
pub mod usb_device;
