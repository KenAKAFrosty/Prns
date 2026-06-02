//! `EmbassyContractHost` — the embassy-executor [`Host`] for the contract runtime.
//!
//! The embassy twin of [`LinuxSync`](super::LinuxSync): it owns the embassy-time
//! clock, an injected CSPRNG draw, and the shared [`WakeSignal`] every interface
//! seam pokes. [`wait`](EmbassyContractHost::wait) genuinely suspends on
//! `select(wake.wait(), Timer::at(deadline))`, so an embassy executor drives the
//! generic [`run_contract`](crate::runtime::run_contract) loop. Inbound flows
//! through the interfaces' handles, drained by the
//! [`ContractRuntime`](crate::runtime::ContractRuntime) — not the host.
//!
//! There is no `clock_base`: the embassy clock is global, and the interface seams
//! stamp `arrived_at` off the same `Instant::now()`, so arrival and the cycle clock
//! already share a timebase with nothing to thread through.

use embassy_futures::select::select;
use embassy_time::{Instant as EmbassyInstant, Timer};

use super::super::{CycleStamp, Host};
use crate::engine::{EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork};
use crate::runtime::channels::embassy_seam::WakeSignal;

/// The embassy contract host: the shared wake the interface seams poke, an injected
/// CSPRNG draw, and the embassy-time clock + sleep. Hand it to
/// `ContractRuntime::new(state, started, host)`, then drive it from an embassy task
/// with `run_contract(runtime, observe).await`. `E` is the host's per-cycle CSPRNG
/// draw (the one substrate piece `personal-rns` can't name itself, e.g. the ESP
/// hardware RNG).
pub struct EmbassyContractHost<E> {
    wake: &'static WakeSignal,
    draw_entropy: E,
}

impl<E> EmbassyContractHost<E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    /// `wake` is the host's one shared signal — every interface seam holds a
    /// `&'static` to it and signals on `submit` / `report`, so a suspended `wait`
    /// returns the moment any interface has something. `draw_entropy` is the device
    /// CSPRNG.
    pub fn new(wake: &'static WakeSignal, draw_entropy: E) -> Self {
        Self { wake, draw_entropy }
    }
}

impl<E> Host for EmbassyContractHost<E>
where
    E: FnMut() -> EngineCycleEntropySeed,
{
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp {
        // Suspend until the engine's next deadline or an interface pokes the wake —
        // whichever first. This is the genuine `.await`: the executor sleeps the core
        // here. The signal is coalesced (a pending poke makes the next wait return at
        // once) and the runtime drains every interface's ring each cycle, so a missed
        // poke only costs the next deadline, never a packet.
        match wake {
            NextScheduledEngineWork::Immediate => {}
            NextScheduledEngineWork::At(deadline) => {
                let at = EmbassyInstant::from_millis(deadline.0);
                let _ = select(self.wake.wait(), Timer::at(at)).await;
            }
            NextScheduledEngineWork::Idle => {
                self.wake.wait().await;
            }
        }

        CycleStamp {
            now: InstantMillis(EmbassyInstant::now().as_millis()),
            seed: (self.draw_entropy)(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ENGINE_CYCLE_ENTROPY_LEN;
    use crate::runtime::channels::embassy_seam::new_wake_signal;
    use core::future::Future;
    use core::pin::{pin, Pin};
    use core::task::{Context, Poll, Waker};
    use embassy_time::{Duration as EmbassyDuration, MockDriver};

    /// A constant per-cycle entropy draw. The seed's contents don't matter to the
    /// wait race — only that the host produces a [`CycleStamp`] when it returns.
    fn const_entropy() -> EngineCycleEntropySeed {
        EngineCycleEntropySeed::new([0x5A; ENGINE_CYCLE_ENTROPY_LEN])
    }

    /// A fresh `&'static WakeSignal` per case. Leaking is fine in a test and keeps
    /// each case's signal state independent of the others.
    fn leak_wake() -> &'static WakeSignal {
        Box::leak(Box::new(new_wake_signal()))
    }

    /// Poll a pinned future once with a no-op waker. The mock driver is advanced
    /// by hand between polls, so we never depend on a real executor or wall clock.
    fn poll_once<F: Future>(fut: Pin<&mut F>) -> Poll<F::Output> {
        fut.poll(&mut Context::from_waker(Waker::noop()))
    }

    // The embassy-time `MockDriver` is a process-global singleton, so every case that
    // touches it must run sequentially. Holding them in one `#[test]` keeps the shared
    // clock deterministic without pulling in `serial_test`. Cases that advance the
    // clock come last so earlier cases observe a clean `now == 0`.
    #[test]
    fn embassy_wait_races_the_deadline_against_the_wake() {
        let driver = MockDriver::get();
        driver.reset(); // now == 0

        // Immediate: a cycle is ready now — `wait` must not suspend, and it stamps the
        // current (un-advanced) clock.
        {
            let wake = leak_wake();
            let mut host = EmbassyContractHost::new(wake, const_entropy);
            let mut fut = pin!(host.wait(NextScheduledEngineWork::Immediate));
            match poll_once(fut.as_mut()) {
                Poll::Ready(stamp) => assert_eq!(stamp.now.0, 0, "stamped at the current clock"),
                Poll::Pending => panic!("Immediate must not suspend"),
            }
        }

        // Idle: no scheduled work and no timer — `wait` suspends on the bare signal and
        // is released only when an interface pokes it.
        {
            let wake = leak_wake();
            let mut host = EmbassyContractHost::new(wake, const_entropy);
            let mut fut = pin!(host.wait(NextScheduledEngineWork::Idle));
            assert!(
                matches!(poll_once(fut.as_mut()), Poll::Pending),
                "Idle suspends while the wake is quiet"
            );
            wake.signal(());
            assert!(
                matches!(poll_once(fut.as_mut()), Poll::Ready(_)),
                "a poke releases the Idle wait"
            );
        }

        // At + pending poke: the race's point on the embassy side — a far deadline must
        // not stop an already-waiting interface from returning at once. The wake arm
        // wins the `select`, the timer never fires, the clock never moves.
        {
            let wake = leak_wake();
            let mut host = EmbassyContractHost::new(wake, const_entropy);
            wake.signal(()); // inbound already queued
            let far = NextScheduledEngineWork::At(InstantMillis(60_000)); // 60s out
            let mut fut = pin!(host.wait(far));
            match poll_once(fut.as_mut()) {
                Poll::Ready(stamp) => {
                    assert_eq!(
                        stamp.now.0, 0,
                        "returned before the deadline, clock unmoved"
                    )
                }
                Poll::Pending => panic!("a pending poke must short-circuit the deadline"),
            }
        }

        // At, no poke: the other side of the race — `wait` suspends until the engine's
        // deadline comes due, then returns. Advancing the mock clock past the deadline
        // is what releases it (no executor, no sleep).
        {
            let wake = leak_wake();
            let mut host = EmbassyContractHost::new(wake, const_entropy);
            let mut fut = pin!(host.wait(NextScheduledEngineWork::At(InstantMillis(100))));
            assert!(
                matches!(poll_once(fut.as_mut()), Poll::Pending),
                "no poke and the deadline is in the future → suspend"
            );
            driver.advance(EmbassyDuration::from_millis(100));
            match poll_once(fut.as_mut()) {
                Poll::Ready(stamp) => assert!(
                    stamp.now.0 >= 100,
                    "woke at {}ms, before the 100ms deadline",
                    stamp.now.0
                ),
                Poll::Pending => panic!("the deadline must release the wait once time advances"),
            }
        }
    }
}
