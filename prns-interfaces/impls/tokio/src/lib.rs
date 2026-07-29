#![forbid(unsafe_code)]

cfg_if::cfg_if! {
    if #[cfg(feature = "log")] {
        #[allow(unused_imports)]
        pub(crate) mod diagnostic_log {
            pub(crate) use log::{debug, error, info, trace, warn};
        }
    } else {
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
    }
}

mod attachment;

pub mod reconnect;

pub mod interface_menu;

#[cfg(feature = "config")]
pub mod from_plan;

#[cfg(feature = "interface-discovery")]
pub mod interface_discovery;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
))]
mod byte_stream;

#[cfg(any(feature = "tcp", feature = "i2p"))]
pub mod tcp;

#[cfg(feature = "udp")]
pub mod udp;

#[cfg(feature = "serial")]
pub mod serial;

#[cfg(feature = "kiss")]
pub mod kiss;

#[cfg(feature = "rnode")]
pub mod rnode;

#[cfg(feature = "pipe")]
pub mod pipe;

#[cfg(feature = "config")]
mod host_network;

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "browser-rendezvous")]
pub mod browser_rendezvous;

#[cfg(feature = "i2p")]
pub mod i2p;

#[cfg(feature = "weave")]
pub mod weave;

#[cfg(feature = "ax25")]
pub mod ax25_kiss;

#[cfg(feature = "backbone")]
pub mod backbone;

#[cfg(feature = "network-device-selection")]
mod network_device;

#[cfg(feature = "wifi-auto")]
pub mod wifi_auto;

#[cfg(feature = "wifi-direct")]
pub mod wifi_direct;

#[cfg(feature = "wifi-aware")]
pub mod wifi_aware;

#[cfg(feature = "usb")]
pub mod usb_auto;

#[cfg(feature = "shared-instance")]
pub mod shared_instance;

#[cfg(feature = "bluetooth-auto")]
pub mod bluetooth_auto;
