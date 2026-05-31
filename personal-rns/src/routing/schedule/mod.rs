//! Pending rebroadcasts of accepted announces — the alloc-free analog of
//! RNS's `announce_table`
//! ([Transport.py:113](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L113)):
//! "a table for storing announces currently waiting to be retransmitted."
//!
//! Each entry covers one destination whose announce we accepted and now owe
//! the network a re-emission of, holding its jittered due time. Keyed by
//! destination, so a fresher announce supersedes the one already waiting
//! rather than queuing a second time.
//!
//! Entries are deliberately tiny: the announce bytes live in the routing
//! table's app_data arena and are read back at emit time, so an entry holds
//! only *which* destination and *when*. That keeps the freshest accepted
//! announce as the one rebroadcast and avoids a second copy of the payload.

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::DEFAULT_MAX_TRACKED_DESTINATIONS;
use crate::wire::DestinationHash;
use heapless::Vec;

/// One scheduled rebroadcast: which destination, when it's due, and the
/// interface the announce we're re-emitting was originally delivered on.
/// The `source_interface` tag is opaque to the engine — it carries
/// through on the [`EgressDirective`](crate::engine::EgressDirective) as
/// `received_from`, so the host can apply RNS's "don't gossip back to
/// source" rule in fanout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRebroadcast {
    pub destination: DestinationHash,
    pub due_at: InstantMillis,
    pub source_interface: InterfaceId,
}

/// A fixed-capacity set of pending rebroadcasts, at most one per destination.
/// Capacity matches the routing table (there is never more than one pending
/// rebroadcast per tracked route).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PendingRebroadcasts<const MAX_PENDING: usize = DEFAULT_MAX_TRACKED_DESTINATIONS> {
    pending: Vec<ScheduledRebroadcast, MAX_PENDING>,
}

impl<const MAX_PENDING: usize> PendingRebroadcasts<MAX_PENDING> {
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
    ) {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|entry| entry.destination == destination)
        {
            // A fresher announce supersedes the queued one — both timing
            // AND the source override (the freshest accept wins for the
            // host's "don't gossip back" decision too).
            existing.due_at = due_at;
            existing.source_interface = source_interface;
        } else {
            // One entry per destination and capacity equals the routing table's, so a
            // newly accepted destination always has room.
            let _ = self.pending.push(ScheduledRebroadcast {
                destination,
                due_at,
                source_interface,
            });
        }
    }

    /// Remove and return one rebroadcast whose time has come (`due_at <= now`),
    /// or `None` if nothing is due yet. The order entries come out in is
    /// unspecified.
    pub fn take_due(&mut self, now: InstantMillis) -> Option<ScheduledRebroadcast> {
        let due = self.pending.iter().position(|entry| entry.due_at <= now)?;
        Some(self.pending.swap_remove(due))
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScheduledRebroadcast> + '_ {
        self.pending.iter()
    }

    /// The earliest `due_at` across all queued rebroadcasts, or `None` if the
    /// queue is empty. This is the queue's contribution to "when must the engine
    /// next wake?" — kept here next to `schedule`/`take_due` so the deadline and
    /// the due-check have a harder time drifting apart.
    pub fn earliest_due_at(&self) -> Option<InstantMillis> {
        self.pending.iter().map(|entry| entry.due_at).min()
    }

    /// Remove every entry whose `due_at <= now`. Returns how many were
    /// removed. Used by `TickOutput`'s Drop to commit a tick's
    /// emissions once the host has iterated them.
    pub fn drain_due(&mut self, now: InstantMillis) -> usize {
        let mut removed = 0;
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].due_at <= now {
                self.pending.swap_remove(i);
                removed += 1;
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Count entries whose `due_at <= now` without removing them.
    pub fn count_due(&self, now: InstantMillis) -> usize {
        self.pending
            .iter()
            .filter(|entry| entry.due_at <= now)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    /// Synthetic source-interface tag — the tests don't care what value
    /// it is, only that it threads through.
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn nothing_is_due_before_its_time_then_it_drains_once() {
        let mut pending = PendingRebroadcasts::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));

        assert_eq!(pending.take_due(InstantMillis(99)), None); // not yet
        assert_eq!(
            pending.take_due(InstantMillis(100)),
            Some(ScheduledRebroadcast {
                destination: dest(1),
                due_at: InstantMillis(100),
                source_interface: iface(0xAA),
            })
        );
        assert_eq!(pending.take_due(InstantMillis(100)), None); // drained
        assert_eq!(pending.pending_count(), 0);
    }

    #[test]
    fn rescheduling_a_destination_updates_time_and_source_without_duplicating() {
        let mut pending = PendingRebroadcasts::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        // A fresher announce, this time arriving via a different interface.
        pending.schedule(dest(1), InstantMillis(200), iface(0xBB));
        assert_eq!(pending.pending_count(), 1);

        assert_eq!(pending.take_due(InstantMillis(150)), None); // moved to 200
        let taken = pending.take_due(InstantMillis(200)).unwrap();
        assert_eq!(taken.destination, dest(1));
        assert_eq!(taken.source_interface, iface(0xBB));
    }

    #[test]
    fn only_entries_whose_time_has_come_are_taken() {
        let mut pending = PendingRebroadcasts::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(300), iface(0xAA));

        assert_eq!(
            pending.take_due(InstantMillis(200)).map(|e| e.destination),
            Some(dest(1))
        );
        assert_eq!(pending.take_due(InstantMillis(200)), None); // dest(2) not due
        assert_eq!(
            pending.take_due(InstantMillis(300)).map(|e| e.destination),
            Some(dest(2))
        );
    }

    #[test]
    fn a_tick_drains_every_due_entry() {
        let mut pending = PendingRebroadcasts::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(3), InstantMillis(100), iface(0xAA));

        let mut drained = std::vec::Vec::new();
        while let Some(entry) = pending.take_due(InstantMillis(100)) {
            drained.push(entry.destination);
        }
        drained.sort_by_key(|d| *d.as_bytes());
        assert_eq!(drained, std::vec![dest(1), dest(2), dest(3)]);
        assert_eq!(pending.pending_count(), 0);
    }
}
