//! Host seam: the platform body the pure engine runs on.
//!
//! The engine is platform-agnostic. Each target (daemon, microcontroller, SDK)
//! supplies a `HostAdapter` providing the clock, the inbound queue, an
//! entropy source, and an egress sink. This trait is the complete inventory
//! of what the stack asks of the world; keep it small.

use crate::engine::{InboundPacket, InstantMillis};
use crate::interfaces::InterfaceId;

pub trait HostAdapter {
    type Error;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error>;

    /// Fill `buf` with CSPRNG-quality random bytes. The engine consumes these
    /// as opaque data. RNS specifies the same bar
    /// (`os.urandom` or better) and we hold to it: a non-CSPRNG host turns
    /// future crypto into latent forgeability bugs the engine cannot detect.
    ///
    /// Canonical implementations:
    /// - std platforms: `getrandom::getrandom(buf)` (the OS RNG shim)
    /// - ESP32-family: `esp_hal::rng::Rng` (hardware RNG peripheral)
    /// - Nordic nRF: `embassy_nrf::rng::Rng` (hardware RNG peripheral)
    ///
    /// Test hosts may seed deterministically (counter, fixed pattern) so
    /// determinism tests can compare byte-identical runs; tests are not
    /// crypto consumers and the engine doesn't enforce the contract at the
    /// trait surface.
    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Drain a batch of queued inbound packets, each stamped with its arrival
    /// instant. The host owns the backing storage and lends the batch for one
    /// `ingest`. Draining need not be exhaustive: the host may cap the batch so
    /// a burst can't make one `step` do unbounded work — the remainder waits for
    /// the next call. An empty slice means nothing is queued.
    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error>;

    /// Handle one outgoing wire packet the engine produced. The runtime
    /// calls this once per directive the tick emitted, passing the
    /// already-serialized bytes and an engine-computed list of positive
    /// `fire_on` targets — exactly the interface ids the host should
    /// transmit on. The list is never empty: the engine elides
    /// empty-fanout directives before they reach the host.
    ///
    /// The host is a pure tx/rx pump: write `bytes` to each id in
    /// `fire_on`. No "skip the source" logic is needed — the engine
    /// already filtered. As more directive variants land (data sends,
    /// proofs, link replies) the engine may compute different `fire_on`
    /// shapes per kind, but the host's job stays the same: take the
    /// list, transmit on each.
    ///
    /// Returning `Err` propagates out of `runtime::step` and halts the
    /// step early. Most hosts will want to log and swallow per-
    /// interface dispatch failures, returning `Ok` so the rest of the
    /// tick's directives still get a chance to dispatch.
    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error>;
}
