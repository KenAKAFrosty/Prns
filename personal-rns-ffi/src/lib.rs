// uniffi's generated scaffolding can trip `clippy::empty_line_after_doc_comments`
// inside the `include_scaffolding!` expansion — silence it at the crate level
// so a `-D warnings` lane doesn't fail on code we don't author.
#![allow(clippy::empty_line_after_doc_comments)]

//! uniffi bindings for `personal-rns`.
//!
//! One UDL describes the SDK surface once; uniffi generates Kotlin,
//! Swift, and Python bindings from it. The `.aar` (Android),
//! `.xcframework` (iOS), and PyPI wheel (Python) packages all consume
//! the cdylib this crate produces.
//!
//! This crate is std-only (uniffi requires std). The `personal-rns`
//! core it wraps stays `no_std` with no allocator; that constraint sits
//! on the core, not on this bindings layer. The SDK is just another
//! engine driver: it brings a thin std-clock adapter and drives the
//! shared engine through `engine::EngineDriver::step`.

use std::sync::Mutex;
use std::time::Instant;

use personal_rns::engine::{EngineDriver, FixedCapacityEngineState, InboundPacket, InstantMillis};
use personal_rns::interfaces::InterfaceId;

uniffi::include_scaffolding!("prns");

/// The personal-rns crate version. Binding consumers assert ABI
/// compatibility against this at startup.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The SDK's thin engine host: a real monotonic clock, but no transport
/// wired yet. So it reports no inbound and refuses to transmit, like
/// every other minimal body until the protocol slices land.
struct SdkEngineDriver {
    base: Instant,
}

#[derive(Debug)]
enum SdkEngineDriverError {
    NoTransport,
    EntropySourceUnavailable,
}

impl EngineDriver for SdkEngineDriver {
    type Error = SdkEngineDriverError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(self.base.elapsed().as_millis() as u64))
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // Same OS CSPRNG path as StdEngineDriver: getrandom backs onto Android's
        // `/dev/urandom` (via `getrandom(2)` on API ≥17) and iOS's
        // `SecRandomCopyBytes`. Crypto-grade by host contract.
        getrandom::getrandom(buf).map_err(|_| SdkEngineDriverError::EntropySourceUnavailable)
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn handle_egress(
        &mut self,
        _bytes: &[u8],
        _fire_on: &[InterfaceId],
    ) -> Result<(), Self::Error> {
        // No transport wired yet; every egress fails honestly until a
        // real interface lands.
        Err(SdkEngineDriverError::NoTransport)
    }
}

struct RuntimeInner {
    state: FixedCapacityEngineState,
    driver: SdkEngineDriver,
}

/// SDK-facing runtime handle. Wraps the pure [`EngineState`] and the
/// SDK's [`SdkEngineDriver`] behind a `Mutex` because uniffi interface objects
/// must be `Send + Sync` and each `tick` mutates engine state.
pub struct ReticulumRuntime {
    inner: Mutex<RuntimeInner>,
}

impl ReticulumRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                state: FixedCapacityEngineState::default(),
                driver: SdkEngineDriver {
                    base: Instant::now(),
                },
            }),
        }
    }

    /// Drive one step over the SDK clock host (ingest the queue, then tick);
    /// returns the directive count the periodic pass emitted.
    pub fn tick(&self) -> u64 {
        let mut inner = self.inner.lock().expect("ReticulumRuntime mutex poisoned");
        let RuntimeInner { state, driver } = &mut *inner;
        let output = driver.step(state).expect("clock-only step cannot fail");
        output.tick.egress_directive_count as u64
    }

    /// Total ticks advanced since construction.
    pub fn tick_count(&self) -> u64 {
        self.inner
            .lock()
            .expect("ReticulumRuntime mutex poisoned")
            .state
            .tick_count()
    }
}

impl Default for ReticulumRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_the_crate_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn runtime_tick_advances_without_transport_emissions() {
        let runtime = ReticulumRuntime::new();

        assert_eq!(runtime.tick_count(), 0);
        assert_eq!(runtime.tick(), 0);
        assert_eq!(runtime.tick_count(), 1);
    }
}
