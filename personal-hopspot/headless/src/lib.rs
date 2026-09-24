//! Shared host attachment options for the application and bounded probe.
#![forbid(unsafe_code)]

#[cfg(feature = "wifi-halow")]
pub mod halow;

#[cfg(feature = "wifi-auto")]
pub mod auto_wifi;

pub mod control;
pub mod control_host;
pub mod tcp;
