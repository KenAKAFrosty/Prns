#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "esp-now")]
extern crate alloc;

#[cfg(feature = "tcp")]
pub mod tcp;

#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(feature = "lora")]
pub mod lora;

#[cfg(feature = "esp-now")]
pub mod esp_now;

#[cfg(feature = "ble")]
pub mod ble;

#[cfg(feature = "usb")]
pub mod usb;

#[cfg(feature = "usb-device")]
pub mod usb_device;
