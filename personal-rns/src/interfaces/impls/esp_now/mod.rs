//! The ESP-NOW interface: a Personal-native broadcast medium carrying Reticulum
//! packets Hopspot-to-Hopspot over the 2.4 GHz radio (ESP-NOW v2). It has no
//! stock-RNS counterpart — both ends are ours — so the framing is ours, and the
//! fat v2 frame (1470 B vs. the 500 B MTU) lets the worker coalesce several
//! packets into one transmission instead of fragmenting.
//!
//! [`core`] is the platform-agnostic part: the routing descriptor and the
//! coalescing frame codec. The embassy worker shell (driving esp-radio's ESP-NOW
//! sender/receiver) lands alongside it for hosts that have the radio.

pub mod core;
