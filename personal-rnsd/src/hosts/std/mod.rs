//! Real-clock host for std platforms.
//!
//! Supplies the monotonic clock and an OS CSPRNG (via `getrandom`) the engine
//! needs. No transport is wired yet, so it reports no inbound packets and
//! refuses to transmit any non-empty batch until a real interface lands —
//! both reported honestly rather than faked. Empty pumps succeed (the engine
//! emits nothing when there's nothing scheduled), so the steady idle loop
//! runs cleanly.

#[cfg(feature = "std-host")]
pub mod usb;

use std::time::Instant;

use personal_rns::engine::{InboundPacket, InstantMillis};
use personal_rns::host::HostAdapter;
use personal_rns::interfaces::InterfaceId;

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
    /// The OS RNG refused to fill the entropy buffer. Vanishingly rare on
    /// Linux/macOS/Windows; surfaced honestly rather than papered over so
    /// crypto callers never see silent zeros.
    EntropySourceUnavailable,
}

impl HostAdapter for StdHost {
    type Error = StdHostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(self.base.elapsed().as_millis() as u64))
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // OS CSPRNG — same source RNS uses (`os.urandom`). Linux/macOS read
        // `getrandom(2)`, Windows reads `BCryptGenRandom`; both are the
        // documented "secure random" entry points.
        getrandom::getrandom(buf).map_err(|_| StdHostError::EntropySourceUnavailable)
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn handle_egress(
        &mut self,
        _bytes: &[u8],
        _fire_on: &[InterfaceId],
    ) -> Result<(), Self::Error> {
        // No transport wired yet — every egress hits this and fails
        // honestly. Once a real interface lands, this method dispatches
        // the bytes per host fanout policy.
        Err(StdHostError::NoTransport)
    }
}
