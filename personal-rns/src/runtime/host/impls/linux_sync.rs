//! `LinuxSync` — the std poll-loop [`Host`].
//!
//! Owns the std substrate a daemon-style host needs: the OS monotonic clock, the
//! OS CSPRNG, and the inbound mailbox the interface worker threads stamp into.
//! [`wait`](LinuxSync::wait) blocks the thread on `recv_timeout` until the
//! engine's next deadline or a stamped packet — the sync-host shape: it never
//! `.await`s, so [`block_on`](super::super::block_on) drives the generic
//! [`run`](crate::runtime::run) loop straight through with no executor.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use std::vec::Vec;

use super::super::{CycleStamp, Host};
use crate::engine::{
    EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledEngineWork,
    ENGINE_CYCLE_ENTROPY_LEN,
};
use crate::runtime::channels::std_host::InboxEntry;

/// Cap on packets ingested per cycle, so a burst can't make one cycle do
/// unbounded work — the rest waits for the next.
const MAX_BATCH: usize = 16;
/// Upper bound on one blocking wait, so a far-off deadline or an idle engine
/// still loops back periodically. A daemon host is mains-powered; the cap is free.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// The std poll-loop host: an OS clock + CSPRNG + the mpsc inbound mailbox the
/// interface worker threads stamp into. Hand it to `Runtime::new(state, workers,
/// host)`, then drive with `block_on(run(runtime, observe))`.
pub struct LinuxSync {
    inbound: Receiver<InboxEntry>,
    clock_base: Instant,
    batch: Vec<InboxEntry>,
}

impl LinuxSync {
    /// `inbound` is the draining end of the mailbox the worker threads stamp
    /// into; `clock_base` is the monotonic reference the workers also measure
    /// arrival stamps against, so `arrived_at` and the cycle clock share a
    /// timebase (pass the same `Instant` the workers were handed).
    pub fn new(inbound: Receiver<InboxEntry>, clock_base: Instant) -> Self {
        Self {
            inbound,
            clock_base,
            batch: Vec::new(),
        }
    }

    fn now(&self) -> InstantMillis {
        InstantMillis(self.clock_base.elapsed().as_millis() as u64)
    }
}

/// How long the next inbound-wait should block: due now → don't block; a future
/// deadline → exactly the gap (capped); idle → the cap.
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
        self.batch.clear();

        // Block until the engine's next deadline or a stamped packet — whichever
        // first. `recv_timeout` blocks the thread (this never `.await`s), so
        // `block_on` carries the run loop straight through with no executor.
        let wait = wait_for(wake, self.now(), MAX_WAIT);
        if !wait.is_zero() {
            if let Ok(entry) = self.inbound.recv_timeout(wait) {
                self.batch.push(entry);
            }
        }
        // Drain whatever else is queued, capped.
        while self.batch.len() < MAX_BATCH {
            match self.inbound.try_recv() {
                Ok(entry) => self.batch.push(entry),
                Err(_) => break,
            }
        }

        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        getrandom::getrandom(&mut seed).expect("OS CSPRNG must provide cycle entropy");
        CycleStamp {
            now: self.now(),
            seed: EngineCycleEntropySeed::new(seed),
        }
    }

    fn inbound_packets(&self) -> impl Iterator<Item = InboundPacket<'_>> {
        self.batch.iter().map(|entry| InboundPacket {
            arrived_at: entry.arrived_at,
            source_interface: entry.source,
            bytes: &entry.bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InterfaceId;
    use crate::runtime::block_on;
    use std::sync::mpsc;

    #[test]
    fn wait_drains_a_stamped_entry_into_inbound() {
        let (tx, rx) = mpsc::channel::<InboxEntry>();
        let mut host = LinuxSync::new(rx, Instant::now());
        tx.send(InboxEntry {
            arrived_at: InstantMillis(5),
            source: InterfaceId::new([0xAB; 16]),
            bytes: std::vec![0xAA, 0xBB],
        })
        .unwrap();

        // Immediate wake → no block; drains the queued entry into the batch.
        let _stamp = block_on(host.wait(NextScheduledEngineWork::Immediate));

        let packets: Vec<InboundPacket<'_>> = host.inbound_packets().collect();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].source_interface, InterfaceId::new([0xAB; 16]));
        assert_eq!(packets[0].bytes, &[0xAA, 0xBB][..]);
    }
}
