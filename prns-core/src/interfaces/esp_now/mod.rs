//! ESP-NOW: a connectionless, broadcast-only WiFi-MAC carrier, embedded-only. The platform-agnostic
//! [`core`] (descriptor, the [`Channel`] newtype + [`ChannelPolicy`],
//! and the [`EspNowRadio`] abstraction the board implements) is always built; the
//! embassy worker that drives it lives in `prns-interfaces-embassy`.

pub mod core;

pub use core::{Channel, ChannelPolicy, EspNowRadio, ESP_NOW_HW_MTU, ESP_NOW_V2_AIR_MTU};
