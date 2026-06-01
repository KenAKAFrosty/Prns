//! The std driver for a [`Manifold`] — threads, std
//! channels, and the OS clock/CSPRNG. The host twin of the `embassy` driver.
//!
//! A worker runs in its own OS thread and stamps [`InboxEntry`]s into the mpsc
//! mailbox this loop drains; [`run`] aggregates that inbound, cycles the engine,
//! routes egress to the worker(s), then blocks until either a stamped packet
//! arrives or the engine's next deadline elapses — the same deadline-driven,
//! no-busy-poll shape as the embassy loop, expressed with `recv_timeout` instead
//! of `select`.
//!
//! The mailbox lives here, with its draining end — the same rule the embassy
//! driver follows. A worker is handed an [`InboundSender`] and stamps into it.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use std::vec::Vec;

use super::super::{Manifold, RuntimeSnapshot};
use crate::engine::{
    EngineCycleEntropySeed, InboundPacket, InstantMillis, NextScheduledEngineWork,
};
use crate::interfaces::{InterfaceId, InterfaceWorker};
use crate::routing::storage::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData, RouteColumns,
};

/// One inbound packet a worker stamped: owned wire bytes plus the provenance the
/// manifold needs (which interface heard it, and when). Owned (not borrowed)
/// because it rides an mpsc channel from the worker thread to the runtime thread.
pub struct InboxEntry {
    pub arrived_at: InstantMillis,
    pub source: InterfaceId,
    pub bytes: Vec<u8>,
}

/// The stamping end a worker holds.
pub type InboundSender = Sender<InboxEntry>;
/// The draining end this driver holds.
pub type InboundReceiver = Receiver<InboxEntry>;

/// Cap on how many stamped packets one cycle ingests, so a burst can't make a
/// single cycle do unbounded work — the remainder waits for the next cycle.
const MAX_BATCH: usize = 16;
/// Upper bound on a single blocking wait, so a far-off deadline or an idle engine
/// still loops back to re-check periodically. A host is mains-powered, so the cap
/// costs nothing; it mirrors the embassy driver's bounded wait.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// How long the next inbound-wait should block, given the engine's next
/// scheduled work and the current time: due now → don't block; a future deadline
/// → exactly the gap (capped at `max`); idle → `max`.
fn wait_until(next: NextScheduledEngineWork, now: InstantMillis, max: Duration) -> Duration {
    match next {
        NextScheduledEngineWork::Immediate => Duration::ZERO,
        NextScheduledEngineWork::Idle => max,
        NextScheduledEngineWork::At(deadline) => {
            Duration::from_millis(deadline.0.saturating_sub(now.0)).min(max)
        }
    }
}

/// Drive `manifold` forever on a std host: aggregate the inbound the worker(s)
/// stamped, cycle the engine, route egress, hand the fresh [`RuntimeSnapshot`] to
/// `on_snapshot`, then block until new inbound arrives or the engine's next
/// deadline. `draw_entropy` is the host's CSPRNG (the one input the manifold
/// can't supply itself); the clock is the std monotonic clock.
///
/// `on_snapshot` is the std analog of the embassy driver's snapshot `Watch`: the
/// app reads engine state through it (a daemon logs route growth; a TUI redraws)
/// without touching engine internals.
///
/// `clock_base` is the monotonic reference both this loop and the worker threads
/// measure `InstantMillis` against, so a packet's `arrived_at` shares the engine's
/// timebase — pass the same `Instant` the workers were handed.
pub fn run<W, R, A, H, D, const MAX_HELD: usize, E, S>(
    mut manifold: Manifold<W, R, A, H, D, MAX_HELD>,
    inbound: InboundReceiver,
    clock_base: Instant,
    mut draw_entropy: E,
    mut on_snapshot: S,
) where
    W: InterfaceWorker,
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
    E: FnMut() -> EngineCycleEntropySeed,
    S: FnMut(&RuntimeSnapshot),
{
    let base = clock_base;
    let mut pending: Option<InboxEntry> = None;
    loop {
        // Aggregate this cycle's batch: carry any pending entry, then drain what's
        // queued (capped). Entries own their bytes; the borrowed `InboundPacket`
        // batch the engine seam wants borrows from them, so both live to the cycle.
        let mut entries: Vec<InboxEntry> = Vec::new();
        if let Some(entry) = pending.take() {
            entries.push(entry);
        }
        while entries.len() < MAX_BATCH {
            match inbound.try_recv() {
                Ok(entry) => entries.push(entry),
                Err(_) => break,
            }
        }
        let batch: Vec<InboundPacket<'_>> = entries
            .iter()
            .map(|e| InboundPacket {
                arrived_at: e.arrived_at,
                source_interface: e.source,
                bytes: &e.bytes,
            })
            .collect();

        let now = InstantMillis(base.elapsed().as_millis() as u64);
        let _ = manifold.cycle_once(now, draw_entropy(), batch.iter().copied());
        on_snapshot(&manifold.snapshot());
        drop(batch);
        drop(entries);

        // Block until the engine's next deadline or a stamped packet — whichever
        // first. No fixed sleep; the engine has no reason to wake otherwise.
        let now = InstantMillis(base.elapsed().as_millis() as u64);
        let wait = wait_until(manifold.next_wakeup(now), now, MAX_WAIT);
        if wait.is_zero() {
            continue;
        }
        match inbound.recv_timeout(wait) {
            Ok(entry) => pending = Some(entry),
            Err(RecvTimeoutError::Timeout) => {}
            // Every worker (and thus every sender) is gone — nothing will ever
            // arrive again; the host is shutting down.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_until_sizes_the_block_to_the_next_obligation() {
        let now = InstantMillis(1_000);
        // Work due now → don't block.
        assert_eq!(
            wait_until(NextScheduledEngineWork::Immediate, now, MAX_WAIT),
            Duration::ZERO
        );
        // Idle → the bounded cap.
        assert_eq!(
            wait_until(NextScheduledEngineWork::Idle, now, MAX_WAIT),
            MAX_WAIT
        );
        // A near deadline → exactly the gap until it.
        assert_eq!(
            wait_until(
                NextScheduledEngineWork::At(InstantMillis(1_200)),
                now,
                MAX_WAIT
            ),
            Duration::from_millis(200)
        );
        // A far deadline → capped at the cap.
        assert_eq!(
            wait_until(
                NextScheduledEngineWork::At(InstantMillis(9_999_999)),
                now,
                MAX_WAIT
            ),
            MAX_WAIT
        );
        // A deadline already in the past → don't block.
        assert_eq!(
            wait_until(
                NextScheduledEngineWork::At(InstantMillis(500)),
                now,
                MAX_WAIT
            ),
            Duration::ZERO
        );
    }
}
