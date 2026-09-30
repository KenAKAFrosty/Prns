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
