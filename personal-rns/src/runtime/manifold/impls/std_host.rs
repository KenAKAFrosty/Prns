//! The std driver for a [`Manifold`](super::super::Manifold) — threads, std
//! channels, and the OS clock/CSPRNG. The host twin of [`embassy`](super::embassy).
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
use crate::engine::{InboundPacket, InstantMillis, NextScheduledWakeup};
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
    E: FnMut() -> u64,
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
        let _ = manifold.cycle(now, draw_entropy(), &batch);
        on_snapshot(&manifold.snapshot());
        drop(batch);
        drop(entries);

        // Block until the engine's next deadline or a stamped packet — whichever
        // first. No fixed sleep; the engine has no reason to wake otherwise.
        let now = InstantMillis(base.elapsed().as_millis() as u64);
        let wait = match manifold.next_wakeup(now) {
            NextScheduledWakeup::Immediate => Duration::ZERO,
            NextScheduledWakeup::Idle => MAX_WAIT,
            NextScheduledWakeup::At(deadline) => {
                Duration::from_millis(deadline.0.saturating_sub(now.0)).min(MAX_WAIT)
            }
        };
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
