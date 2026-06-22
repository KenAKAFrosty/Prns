//! Interfaces that match an RNS 1.3.5 interface on the wire — TCP, UDP, serial, and (resurrecting)
//! the WiFi/LAN `AutoInterface`. Distinct from our own interfaces (e.g. `usb_auto`), which have no
//! Reticulum counterpart and live at the `interfaces` root.

#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod kiss;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod local;
pub mod lora;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod serial;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod tcp;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod udp;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod wifi_auto;
