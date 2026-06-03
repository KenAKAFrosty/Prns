use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use crate::engine::{EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork};

/// The clock reading and entropy seed a [`Host`] samples at wake, handed to one cycle
/// of the runtime. Owned (not borrowed) so the runner can move the move-only seed
/// straight into the cycle.
pub struct CycleStamp {
    pub now: InstantMillis,
    pub seed: EngineCycleEntropySeed,
}

/// A host: the few substrate methods a platform implements so the runtime can drive
/// it. The host owns the real clock, the CSPRNG, the shared wake, and the sleep
/// primitive — whatever the runtime needs from the outside world flows *up* to it. The
/// [`Runtime`](crate::runtime::Runtime) asks the host for one cycle's
/// fuel at a time via [`Runtime::run`](crate::runtime::Runtime::run); inbound flows
/// through the interfaces' seams, drained by the runtime, not the host.
///
/// `async` because that is the unifying substrate: an executor-backed host suspends in
/// [`wait`](Self::wait); a sync poll-loop host blocks there and never yields, so
/// [`block_on`] drives it with no executor.
#[allow(async_fn_in_trait)]
pub trait Host {
    /// Sleep until the engine's next scheduled work (`wake`) is due or an interface
    /// pokes the shared wake, then sample the clock and draw a fresh per-cycle seed.
    /// The only `.await` in the loop: where an executor-backed host suspends and a sync
    /// host blocks.
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp;
}

/// Drive a runtime future on a substrate with no async executor — a sync poll-loop
/// host. Such a host's [`Host::wait`] blocks internally and never yields `Pending`, so
/// the future runs straight through; the noop waker is never actually scheduled. An
/// executor-backed host drives [`Runtime::run`](crate::runtime::Runtime::run) with its
/// own executor and never calls this.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_a_ready_future_to_completion() {
        assert_eq!(block_on(async { 40 + 2 }), 42);
    }

    #[test]
    fn block_on_repolls_past_a_pending() {
        // Yields `Pending` once (without scheduling a waker), then `Ready` — proves
        // `block_on` re-polls rather than parking forever.
        struct YieldOnce(bool);
        impl Future for YieldOnce {
            type Output = u8;
            fn poll(mut self: core::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u8> {
                if self.0 {
                    Poll::Ready(7)
                } else {
                    self.0 = true;
                    Poll::Pending
                }
            }
        }
        assert_eq!(block_on(YieldOnce(false)), 7);
    }
}
