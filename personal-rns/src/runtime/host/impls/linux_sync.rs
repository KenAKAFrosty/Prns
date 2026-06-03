//! `LinuxSync` — the std poll-loop [`Host`] for the runtime.
//!
//! Owns the std substrate a daemon-style host needs: the OS monotonic clock, the
//! OS CSPRNG, and the wake the interface worker threads poke through their seams.
//! [`wait`](LinuxSync::wait) blocks the thread on `recv_timeout` until the engine's
//! next deadline or an interface signals it has something — the sync-host shape: it
//! never `.await`s, so [`block_on`](super::super::block_on) drives the generic
//! [`Runtime::run`](crate::runtime::Runtime::run) loop straight through with no
//! executor. Inbound does not flow through the host: the
//! [`Runtime`](crate::runtime::Runtime) drains each interface's handle.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::{Duration, Instant};

use super::super::{CycleStamp, Host};
use crate::engine::{
    EngineCycleEntropySeed, InstantMillis, NextScheduledEngineWork, ENGINE_CYCLE_ENTROPY_LEN,
};
use crate::interfaces::substrate::{StdHostSubstrate, StdInterfaceHandle, StdInterfaceSeam};
use crate::interfaces::{Interface, InterfaceId, StartedInterface};

/// Upper bound on one blocking wait, so a far-off deadline or an idle engine still
/// loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// The std poll-loop host: an OS clock + CSPRNG + the one wake the interface seams
/// poke. It owns both the clock and the wake; glue each interface's seam from it
/// with [`glue_seam`](LinuxSync::glue_seam), then hand it to
/// `Runtime::new(state, started, host)` and drive with
/// `block_on(runtime.run(on_snapshot))`.
pub struct LinuxSync {
    wake: Receiver<()>,
    wake_sender: SyncSender<()>,
    clock_base: Instant,
}

impl LinuxSync {
    /// Owns the monotonic clock and the one wake channel. Every seam glued from this
    /// host shares its `clock_base` (so `arrived_at` and the cycle clock share a
    /// timebase) and a clone of its wake sender (so any submit/report rouses a
    /// blocked `wait`).
    pub fn new() -> Self {
        let (wake_sender, wake) = sync_channel::<()>(1);
        Self {
            wake,
            wake_sender,
            clock_base: Instant::now(),
        }
    }

    /// Glue an interface seam bound to this host's clock + wake: the worker takes the
    /// context, the runtime keeps the handle. `MTU` is inferred from how the worker
    /// context is used (the serial interface pins it to `SERIAL_MTU`).
    pub fn glue_seam<const MTU: usize>(&self, id: InterfaceId, depth: usize) -> StdInterfaceSeam<MTU> {
        StdInterfaceSeam::new(id, self.clock_base, depth, self.wake_sender.clone())
    }

    /// Attach `interface` to this host and hand back the [`StartedInterface`] the
    /// runtime pools: glue a seam keyed by the id the interface already carries, then
    /// start the interface onto it. The `glue_seam` + `start_interface` dance,
    /// collapsed — reach for `glue_seam` directly only when a host splits the seam by
    /// hand (a multi-interface board unifying heterogeneous handles).
    pub fn attach<I, const MTU: usize>(
        &self,
        interface: I,
        depth: usize,
    ) -> StartedInterface<StdInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<StdHostSubstrate<MTU>>,
    {
        let id = interface.descriptor().id;
        self.glue_seam(id, depth).start_interface(interface)
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

impl Default for LinuxSync {
    fn default() -> Self {
        Self::new()
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
    use crate::interfaces::InboundSink;
    use crate::runtime::block_on;

    #[test]
    fn wait_returns_promptly_when_an_interface_pokes_the_wake() {
        let mut host = LinuxSync::new();
        // A seam glued from the host shares its wake; a submit pokes that wake, so an
        // `Idle` wait (which would otherwise block for the cap) returns at once.
        let mut seam = host.glue_seam::<8>(InterfaceId::new([0; 16]), 1);
        seam.worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 1;
                1
            })
            .unwrap();
        let _stamp = block_on(host.wait(NextScheduledEngineWork::Idle));
    }
}
