//! Interfaces built fresh against the reactor's [`InterfaceSeam`](super::interface_seam):
//! async-native, each its own small reactor over one medium. They reuse the shared codecs
//! (e.g. `rns_serial_framing`) from `crate::interfaces` but none of the old poll-loop seam.

#[cfg(feature = "tokio-host")]
pub mod framed_stream;
pub mod serial;
pub mod tcp;
pub mod udp;
pub mod usb_auto;
