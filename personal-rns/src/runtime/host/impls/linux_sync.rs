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

    #[test]
    fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let mut host = LinuxSync::new(wake_rx, Instant::now());

        // A pending poke → an `Idle` wait (which would otherwise block for the cap)
        // returns at once.
        wake_tx.try_send(()).unwrap();
        let _stamp = block_on(host.wait(NextScheduledEngineWork::Idle));
    }
}
