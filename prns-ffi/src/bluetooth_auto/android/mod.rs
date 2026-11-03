mod backend;
mod bridge;
mod link;
mod outbound;

#[cfg(test)]
mod tests;

pub use backend::AndroidBleBackend;
pub use bridge::{AndroidBleBridge, AndroidBleIngressAdmission};
pub use link::{AndroidBleControl, AndroidBleLink, AndroidBleSink, AndroidBleSource};
pub use outbound::{AndroidBleControlOutput, AndroidBleControlTicket};
pub use prns_core::interfaces::bluetooth_auto::{
    supports_liveness_capability, LivenessMode, LIVENESS_CAPABILITY_BYTES,
};

pub const LIVENESS_UUID_BYTES: [u8; 16] =
    match prns_core::interfaces::bluetooth_auto::NATIVE_LIVENESS_UUID {
        prns_core::interfaces::bluetooth_auto::BleUuid::Bit128(bytes) => bytes,
        _ => panic!("native liveness requires a 128-bit UUID"),
    };

pub const CONTROL_BUFFER_LEN: usize = prns_core::interfaces::bluetooth_auto::CONTROL_MAX_LEN;

pub const RADIO_ENABLED: u32 = 0x01;
pub const RADIO_ADVERTISING: u32 = 0x02;
pub const RADIO_SCANNING: u32 = 0x04;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AndroidBleError {
    Closed,
    ControlTooLarge,
    FrameTooLarge,
    QueueFull,
    ControlBusy,
    ControlWriteFailed,
    ControlTimedOut,
}
