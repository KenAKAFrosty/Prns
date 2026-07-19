mod backend;
mod bridge;
mod link;

#[cfg(test)]
mod tests;

pub use backend::AndroidBleBackend;
pub use bridge::AndroidBleBridge;
pub use link::{AndroidBleLink, AndroidBleSink, AndroidBleSource};

pub const RADIO_ENABLED: u32 = 0x01;
pub const RADIO_ADVERTISING: u32 = 0x02;
pub const RADIO_SCANNING: u32 = 0x04;

#[derive(Debug)]
pub enum AndroidBleError {
    Closed,
    ControlTooLarge,
    FrameTooLarge,
}
