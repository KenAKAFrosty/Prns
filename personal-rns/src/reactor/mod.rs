//! The async reactor shell: the driver that races the reactor's three inputs — a
//! command, an inbound packet, the next scheduled deadline — runs the one sync engine
//! method the winner names, and turns its [`EngineReaction`](crate::engine::EngineReaction)s
//! into I/O. The engine stays sync and pure; this is the only part that touches time,
//! entropy, and the outside world. It grows here beside the legacy `runtime` module
//! while that one is retired.

use crate::engine::InstantMillis;

/// Everything the reactor needs from the outside world: time and entropy. The reactor is
/// pure — it never reads a clock itself, so the host owns the clock entirely (a test or a
/// deterministic sim hands it any `now` it likes). `tokio` and `embassy` are the two
/// impls; that std-vs-no_std split is the only host distinction left.
#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> InstantMillis;
    /// Suspend until `deadline` — the timer racer in the reactor's select.
    async fn sleep_until(&self, deadline: InstantMillis);
    fn fill_entropy(&mut self, bytes: &mut [u8]);
}

pub mod airtime;
pub mod announce_pacer;
pub mod duty_gate;
pub mod throughput;
pub(crate) mod window_ring;

pub mod grant;

#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
mod driver;

pub mod interface_seam;

#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod impls;
