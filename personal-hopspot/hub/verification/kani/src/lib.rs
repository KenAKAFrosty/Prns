#![no_std]

#[path = "../../../core/src/domain_primitives/device/mod.rs"]
mod device;

pub use device::{DeviceId, DeviceLabel, DeviceLabelError, MAX_DEVICE_LABEL_BYTES};
