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

mod attach;

pub mod reconnect;

pub mod interface_menu;

#[cfg(feature = "auto")]
pub mod auto;

#[cfg(feature = "from-plan")]
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
mod framed_stream;

#[cfg(any(feature = "kiss", feature = "ax25", feature = "rnode"))]
mod kiss_deadline;

#[cfg(any(feature = "tcp", feature = "i2p"))]
pub mod tcp;

#[cfg(feature = "udp")]
pub mod udp;

#[cfg(feature = "serial")]
pub mod serial;

#[cfg(feature = "serial-host")]
pub mod serial_host;

#[cfg(feature = "kiss")]
pub mod kiss;

#[cfg(feature = "rnode")]
pub mod rnode;
#[cfg(feature = "rnode-ble")]
mod rnode_ble;
#[cfg(feature = "from-plan")]
mod rnode_host;

#[cfg(feature = "rnode")]
pub mod rnode_multi;

#[cfg(feature = "pipe")]
pub mod pipe;

#[cfg(feature = "pipe-host")]
pub mod pipe_host;

#[cfg(feature = "from-plan")]
mod host_network;

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "i2p")]
pub mod i2p;

#[cfg(feature = "ax25")]
pub mod ax25;

#[cfg(feature = "backbone")]
pub mod backbone;

#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(feature = "wifi-direct")]
pub mod wifi_direct;

#[cfg(feature = "wifi-aware")]
pub mod wifi_aware;

#[cfg(feature = "usb")]
pub mod usb;

#[cfg(feature = "usb-host")]
pub mod usb_host;

#[cfg(feature = "shared-instance")]
pub mod shared_instance;

#[cfg(feature = "ble")]
pub mod ble;

#[cfg(feature = "ble-host")]
pub mod ble_host;
