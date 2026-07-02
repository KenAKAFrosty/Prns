#![forbid(unsafe_code)]

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "pipe",
    feature = "local",
    feature = "backbone"
))]
mod framed_stream;

#[cfg(feature = "tcp")]
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

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "ax25")]
pub mod ax25;

#[cfg(feature = "backbone")]
pub mod backbone;

#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(feature = "usb")]
pub mod usb;

#[cfg(feature = "shared-instance")]
pub mod shared_instance;

#[cfg(feature = "ble")]
pub mod ble;
