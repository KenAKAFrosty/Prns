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
//! on the core, not on this bindings layer. The SDK brings a thin
//! std-clock/entropy adapter and drives the shared engine directly.

use std::sync::Mutex;
use std::time::Instant;

use personal_rns::engine::{
    EngineCycleEntropy, EngineCycleEntropySeed, EngineState, InstantMillis,
    ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::routing::storage::FixedCapacity;

/// This SDK binding's engine-storage sizing (the engine has no storage defaults, so
/// each consumer picks its own): 64 dests / 64 ids each / 4 KB arena / 4 floor / 512
/// overflow / 64 held.
type SdkEngineStorage = FixedCapacity<64, 64, 4096, 4, 512, 64, 8>;

uniffi::include_scaffolding!("prns");

/// The personal-rns crate version. Binding consumers assert ABI
/// compatibility against this at startup.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The SDK's thin engine substrate: a real monotonic clock and CSPRNG, but no
/// transport wired yet.
struct SdkEngineSubstrate {
    base: Instant,
}

impl SdkEngineSubstrate {
    fn now_millis(&self) -> InstantMillis {
        InstantMillis(self.base.elapsed().as_millis() as u64)
    }

    fn cycle_entropy(&self) -> EngineCycleEntropy {
        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        // Same OS CSPRNG path as the std host: getrandom backs onto Android's
        // `/dev/urandom` (via `getrandom(2)` on API ≥17) and iOS's
        // `SecRandomCopyBytes`. Crypto-grade by host contract.
        getrandom::getrandom(&mut seed).expect("OS CSPRNG must provide cycle entropy");
        EngineCycleEntropy::from_seed(EngineCycleEntropySeed::new(seed))
    }
}

struct RuntimeInner {
    state: EngineState<SdkEngineStorage>,
    substrate: SdkEngineSubstrate,
}

/// SDK-facing runtime handle. Wraps the pure engine state and the SDK's thin
/// substrate behind a `Mutex` because uniffi interface objects
/// must be `Send + Sync` and each `tick` mutates engine state.
pub struct ReticulumRuntime {
    inner: Mutex<RuntimeInner>,
}

impl ReticulumRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                state: EngineState::<SdkEngineStorage>::default(),
                substrate: SdkEngineSubstrate {
                    base: Instant::now(),
                },
            }),
        }
    }

    /// Drive one periodic tick over the SDK clock/entropy substrate; returns
    /// the directive count the periodic pass emitted.
    pub fn tick(&self) -> u64 {
        let mut inner = self.inner.lock().expect("ReticulumRuntime mutex poisoned");
        let RuntimeInner { state, substrate } = &mut *inner;
        let now = substrate.now_millis();
        let entropy = substrate.cycle_entropy();
        let output = state.tick(now, entropy.jitter);
        output.egress_directive_count() as u64
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
