//! `LinuxSync` — the std poll-loop [`Host`] for the contract runtime.
//!
//! Owns the std substrate a daemon-style host needs: the OS monotonic clock, the
//! OS CSPRNG, and the wake the interface worker threads poke through their seams.
//! [`wait`](LinuxSync::wait) blocks the thread on `recv_timeout` until the engine's
//! next deadline or an interface signals it has something — the sync-host shape: it
//! never `.await`s, so [`block_on`](super::super::block_on) drives the generic
//! [`run_contract`](crate::runtime::run_contract) loop straight through with no
//! executor. Inbound does not flow through the host: the
//! [`ContractRuntime`](crate::runtime::ContractRuntime) drains each interface's handle.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::super::{CycleStamp, Host};
use crate::engine::{
    EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork, ENGINE_CYCLE_ENTROPY_LEN,
};

/// Upper bound on one blocking wait, so a far-off deadline or an idle engine still
/// loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// The std poll-loop host: an OS clock + CSPRNG + the wake receiver the interface
/// worker threads poke (through the wake baked into their inbound/report
/// producers). Hand it to `ContractRuntime::new(state, started, host)`, then drive
/// with `block_on(run_contract(runtime, observe))`.
pub struct LinuxSync {
    wake: Receiver<()>,
    clock_base: Instant,
}

impl LinuxSync {
    /// `wake` is the receiving end of the seam wake — every interface's seam holds
    /// a sender that fires on `submit` / `report`, so a blocked `wait` returns the
    /// moment any interface has something. `clock_base` is the monotonic reference
    /// the interfaces also stamp arrival against (pass the same `Instant` handed to
    /// every seam), so `arrived_at` and the cycle clock share a timebase.
    pub fn new(wake: Receiver<()>, clock_base: Instant) -> Self {
        Self { wake, clock_base }
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

/// How long the next wait should block: due now → don't block; a future deadline →
/// exactly the gap (capped); idle → the cap.
fn wait_for(next: NextScheduledEngineWork, now: InstantMillis, max: Duration) -> Duration {
    match next {
        NextScheduledEngineWork::Immediate => Duration::ZERO,
        NextScheduledEngineWork::Idle => max,
        NextScheduledEngineWork::At(deadline) => {
            Duration::from_millis(deadline.0.saturating_sub(now.0)).min(max)
        }
    }
}

impl Host for LinuxSync {
    async fn wait(&mut self, wake: NextScheduledEngineWork) -> CycleStamp {
        // Block until the engine's next deadline or an interface pokes the wake —
        // whichever first. `recv_timeout` blocks the thread (this never `.await`s),
        // so `block_on` carries the loop straight through with no executor. The
        // wake is coalesced (a pending poke just returns immediately); the runtime
        // drains every interface's ring each cycle regardless, so a missed poke
        // only costs the next deadline, never a packet.
        let timeout = wait_for(wake, self.now(), MAX_WAIT);
        if !timeout.is_zero() {
            let _ = self.wake.recv_timeout(timeout);
        }

        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        getrandom::getrandom(&mut seed).expect("OS CSPRNG must provide cycle entropy");
        CycleStamp {
            now: self.now(),
            seed: EngineCycleEntropySeed::new(seed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::block_on;
    use std::sync::mpsc::sync_channel;

    // --- `wait_for`: the pure deadline math (time is an argument, so no clock here) ---

    #[test]
    fn wait_for_immediate_never_blocks() {
        // Work is ready now: don't sleep, drive the next cycle straight away.
        assert_eq!(
            wait_for(
                NextScheduledEngineWork::Immediate,
                InstantMillis(123),
                MAX_WAIT
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn wait_for_idle_blocks_for_the_cap() {
        // No scheduled work: park for the cap so an idle engine still circles back.
        assert_eq!(
            wait_for(NextScheduledEngineWork::Idle, InstantMillis(123), MAX_WAIT),
            MAX_WAIT
        );
    }

    #[test]
    fn wait_for_a_future_deadline_blocks_exactly_the_gap() {
        // 250ms out, well under the cap → wait precisely the gap, no more.
        let now = InstantMillis(1_000);
        let next = NextScheduledEngineWork::At(InstantMillis(1_250));
        assert_eq!(wait_for(next, now, MAX_WAIT), Duration::from_millis(250));
    }

    #[test]
    fn wait_for_a_deadline_already_due_never_blocks() {
        // Deadline exactly now and a deadline already in the past both saturate to
        // zero, so a due-or-overdue deadline runs the next cycle without sleeping —
        // never a negative gap, never a backwards wait.
        let now = InstantMillis(5_000);
        assert_eq!(
            wait_for(
                NextScheduledEngineWork::At(InstantMillis(5_000)),
                now,
                MAX_WAIT
            ),
            Duration::ZERO,
            "a deadline at `now` is due, not a wait"
        );
        assert_eq!(
            wait_for(
                NextScheduledEngineWork::At(InstantMillis(4_000)),
                now,
                MAX_WAIT
            ),
            Duration::ZERO,
            "an overdue deadline saturates to zero, it does not underflow"
        );
    }

    #[test]
    fn wait_for_a_far_deadline_is_capped() {
        // A deadline an hour out must not park the thread for an hour: the cap wins,
        // so the loop keeps circling back every MAX_WAIT.
        let now = InstantMillis(0);
        let next = NextScheduledEngineWork::At(InstantMillis(3_600_000));
        assert_eq!(wait_for(next, now, MAX_WAIT), MAX_WAIT);
    }

    // --- `wait`: the live race — wake on the deadline OR an inbound poke ---

    #[test]
    fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let mut host = LinuxSync::new(wake_rx, Instant::now());

        // A pending poke → an `Idle` wait (which would otherwise block for the cap)
        // returns at once.
        wake_tx.try_send(()).unwrap();
        let _stamp = block_on(host.wait(NextScheduledEngineWork::Idle));
    }

    #[test]
    fn wait_short_circuits_a_far_deadline_when_a_poke_is_pending() {
        // The race's whole point: an interface that already has something must not
        // wait out the engine's next deadline. Pre-queue the poke, set a deadline so
        // far off `wait_for` would otherwise block the full cap, and prove the poke
        // still releases the wait immediately.
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let mut host = LinuxSync::new(wake_rx, Instant::now());
        wake_tx.try_send(()).unwrap();

        let start = Instant::now();
        let _stamp = block_on(host.wait(NextScheduledEngineWork::At(InstantMillis(u64::MAX))));
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "a pending poke must short-circuit a far deadline, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn wait_blocks_for_the_deadline_when_no_poke_arrives() {
        // The other side of the race: with nothing inbound, the wait sleeps until the
        // engine's deadline and no further. Keep the sender alive — a dropped sender
        // disconnects the channel and makes `recv_timeout` return at once, which would
        // mask the very timeout under test.
        let (_wake_tx, wake_rx) = sync_channel::<()>(1);
        let base = Instant::now();
        let mut host = LinuxSync::new(wake_rx, base);

        let start = Instant::now();
        let stamp = block_on(host.wait(NextScheduledEngineWork::At(InstantMillis(120))));
        let elapsed = start.elapsed();

        // Slept until at least the deadline: the engine clock sampled at wake is past
        // it, and real wall-clock was spent (not an instant fall-through).
        assert!(
            stamp.now.0 >= 120,
            "woke at {}ms, before the 120ms deadline",
            stamp.now.0
        );
        assert!(
            elapsed >= Duration::from_millis(60),
            "barely waited: {:?}",
            elapsed
        );
        // ...but the deadline, not the 1s cap, is what released it.
        assert!(elapsed < MAX_WAIT, "fell through to the cap: {:?}", elapsed);
    }

    #[test]
    fn wait_does_not_block_on_immediate_even_with_an_empty_wake() {
        // `Immediate` means a cycle is ready now: `wait` must skip the blocking recv
        // entirely (zero timeout), so an empty, never-poked wake can't stall it.
        let (_wake_tx, wake_rx) = sync_channel::<()>(1);
        let mut host = LinuxSync::new(wake_rx, Instant::now());

        let start = Instant::now();
        let _stamp = block_on(host.wait(NextScheduledEngineWork::Immediate));
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "Immediate must not block, took {:?}",
            start.elapsed()
        );
    }
}
