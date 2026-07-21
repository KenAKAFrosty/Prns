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

pub mod bluetooth_auto;
pub mod mdns;
pub mod wifi_aware;
pub mod wifi_direct;

#[cfg(target_os = "windows")]
pub mod usb_hotplug;
