//! ESP-NOW: a connectionless, broadcast-only WiFi-MAC carrier, embedded-only. The platform-agnostic
//! [`core`] (descriptor, the [`Channel`](core::Channel) newtype + [`ChannelPolicy`](core::ChannelPolicy),
//! and the [`EspNowRadio`](core::EspNowRadio) abstraction the board implements) is always built; the
//! embassy worker that drives it lives behind `embassy-espnow`.

pub mod core;
pub mod impls;

pub use core::{Channel, ChannelPolicy, EspNowRadio, ESP_NOW_HW_MTU, ESP_NOW_V2_AIR_MTU};

#[cfg(feature = "embassy-espnow")]
pub use impls::embassy::EspNowInterface;
