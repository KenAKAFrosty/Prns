#![forbid(unsafe_code)]

mod attach;

pub mod interface_menu;

#[cfg(feature = "auto")]
pub mod auto;

#[cfg(feature = "from-plan")]
pub mod from_plan;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
mod framed_stream;

#[cfg(feature = "tcp")]
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

#[cfg(feature = "pipe")]
pub mod pipe;

#[cfg(feature = "pipe-host")]
pub mod pipe_host;

#[cfg(feature = "websocket")]
pub mod websocket;

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
