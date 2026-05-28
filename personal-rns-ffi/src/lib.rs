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
//! Host: it brings a thin std-clock adapter and drives the shared
//! engine through `runtime::step`.

use std::sync::Mutex;
use std::time::Instant;

use personal_rns::engine::{DefaultEngineState, InboundPacket, InstantMillis, OutboundPacket};
use personal_rns::host::HostAdapter;
use personal_rns::runtime::step;

uniffi::include_scaffolding!("prns");

/// The personal-rns crate version. Binding consumers assert ABI
/// compatibility against this at startup.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The SDK's thin Host adapter: a real monotonic clock, but no transport
/// wired yet. So it reports no inbound and refuses to transmit, like
/// every other minimal body until the protocol slices land.
struct SdkHost {
    base: Instant,
}

#[derive(Debug)]
enum SdkHostError {
    NoTransport,
}

impl HostAdapter for SdkHost {
    type Error = SdkHostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(self.base.elapsed().as_millis() as u64))
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn pump_outbound_packets(
        &mut self,
        _packets: &[OutboundPacket<'_>],
    ) -> Result<(), Self::Error> {
        Err(SdkHostError::NoTransport)
    }
}

struct RuntimeInner {
    state: DefaultEngineState,
    host: SdkHost,
}

/// SDK-facing runtime handle. Wraps the pure [`EngineState`] and the
/// SDK's [`SdkHost`] behind a `Mutex` because uniffi interface objects
/// must be `Send + Sync` and each `tick` mutates engine state.
pub struct ReticulumRuntime {
    inner: Mutex<RuntimeInner>,
}

impl ReticulumRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                state: DefaultEngineState::default(),
                host: SdkHost {
                    base: Instant::now(),
                },
            }),
        }
    }

    /// Drive one step over the SDK clock host (ingest the queue, then tick);
    /// returns the packet count the periodic pass emitted.
    pub fn tick(&self) -> u64 {
        let mut inner = self.inner.lock().expect("ReticulumRuntime mutex poisoned");
        let RuntimeInner { state, host } = &mut *inner;
        let output = step(state, host).expect("clock-only step cannot fail");
        output.tick.emitted_packet_count() as u64
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
