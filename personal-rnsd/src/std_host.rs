//! Real-clock host for std platforms.
//!
//! Supplies the monotonic clock the engine needs. No transport is wired yet, so
//! it reports no inbound packets and refuses to transmit until a real interface
//! lands — both reported honestly rather than faked.

use std::time::Instant;

use personal_rns::engine::{InboundPacket, InstantMillis};
use personal_rns::host::Host;

pub struct StdHost {
    base: Instant,
}

impl StdHost {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for StdHost {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdHostError {
    /// Transmission was requested, but no transport is configured yet.
    NoTransport,
}

impl Host for StdHost {
    type Error = StdHostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(self.base.elapsed().as_millis() as u64))
    }

    fn drain_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn transmit_packet(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
        Err(StdHostError::NoTransport)
    }
}
