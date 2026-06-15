//! Interfaces that match an RNS 1.3.1 interface on the wire — TCP, UDP, serial, and (resurrecting)
//! the WiFi/LAN `AutoInterface`. Distinct from our own interfaces (e.g. `usb_auto`), which have no
//! Reticulum counterpart and live at the `interfaces` root.

pub mod serial;
pub mod tcp;
pub mod udp;
