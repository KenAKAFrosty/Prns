//! Host seam: the platform body the pure engine runs on.
//!
//! The engine is platform-agnostic. Each target (daemon, microcontroller, SDK)
//! supplies a `Host` providing the clock, inbound bytes, and outbound
//! transmission. This trait is the complete inventory of what the stack asks of
//! the world; keep it small.

use crate::engine::InstantMillis;

/// Platform body for the pure Reticulum engine.
pub trait Host {
    type Error;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error>;

    fn receive_packet(&mut self, buffer: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    fn transmit_packet(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}
