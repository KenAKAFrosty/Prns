//! The rebroadcast schedule: destinations whose announce we accepted and now
//! owe the network a re-emission of, each waiting for its jittered moment.
//!
//! This is the alloc-free analog of RNS's `announce_table` — pending
//! retransmits keyed by destination, so a fresher announce supersedes the one
//! already waiting rather than queuing a second time.
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1896-L1906>
//!
//! Entries are deliberately tiny: the announce bytes live in the routing table's
//! payload store and are read back at emit time, so a scheduled entry holds only
//! *which* destination and *when*. That keeps the freshest accepted announce as
//! the one rebroadcast and avoids a second copy of the payload.

use crate::engine::InstantMillis;
use crate::routing::DEFAULT_MAX_TRACKED_DESTINATIONS;
use crate::wire::DestinationHash;
use heapless::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRebroadcast {
    pub destination: DestinationHash,
    pub due_at: InstantMillis,
}

/// A fixed-capacity set of pending rebroadcasts, at most one per destination.
/// Capacity matches the routing table — there is never more than one pending
/// rebroadcast per tracked path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RebroadcastSchedule<const MAX_PENDING: usize = DEFAULT_MAX_TRACKED_DESTINATIONS> {
    pending: Vec<ScheduledRebroadcast, MAX_PENDING>,
}

impl<const MAX_PENDING: usize> RebroadcastSchedule<MAX_PENDING> {
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn schedule(&mut self, destination: DestinationHash, due_at: InstantMillis) {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|entry| entry.destination == destination)
        {
            existing.due_at = due_at;
        } else {
            // One entry per destination and capacity equals the routing table's, so a
            // newly accepted destination always has room.
            let _ = self.pending.push(ScheduledRebroadcast {
                destination,
                due_at,
            });
        }
    }

    /// Remove and return one rebroadcast whose time has come (`due_at <= now`),
    /// or `None` if nothing is due yet. Called in a loop to drain a tick's due
    /// set. The order entries come out in is unspecified.
    pub fn take_due(&mut self, now: InstantMillis) -> Option<ScheduledRebroadcast> {
        let due = self.pending.iter().position(|entry| entry.due_at <= now)?;
        Some(self.pending.swap_remove(due))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn nothing_is_due_before_its_time_then_it_drains_once() {
        let mut schedule = RebroadcastSchedule::<4>::new();
        schedule.schedule(dest(1), InstantMillis(100));

        assert_eq!(schedule.take_due(InstantMillis(99)), None); // not yet
        assert_eq!(
            schedule.take_due(InstantMillis(100)),
            Some(ScheduledRebroadcast {
                destination: dest(1),
                due_at: InstantMillis(100),
            })
        );
        assert_eq!(schedule.take_due(InstantMillis(100)), None); // drained
        assert_eq!(schedule.pending_count(), 0);
    }

    #[test]
    fn rescheduling_a_destination_updates_its_time_without_duplicating() {
        let mut schedule = RebroadcastSchedule::<4>::new();
        schedule.schedule(dest(1), InstantMillis(100));
        schedule.schedule(dest(1), InstantMillis(200)); // a fresher announce supersedes
        assert_eq!(schedule.pending_count(), 1);

        assert_eq!(schedule.take_due(InstantMillis(150)), None); // moved to 200
        assert_eq!(
            schedule.take_due(InstantMillis(200)).map(|e| e.destination),
            Some(dest(1))
        );
    }

    #[test]
    fn only_entries_whose_time_has_come_are_taken() {
        let mut schedule = RebroadcastSchedule::<4>::new();
        schedule.schedule(dest(1), InstantMillis(100));
        schedule.schedule(dest(2), InstantMillis(300));

        assert_eq!(
            schedule.take_due(InstantMillis(200)).map(|e| e.destination),
            Some(dest(1))
        );
        assert_eq!(schedule.take_due(InstantMillis(200)), None); // dest(2) not due
        assert_eq!(
            schedule.take_due(InstantMillis(300)).map(|e| e.destination),
            Some(dest(2))
        );
    }

    #[test]
    fn a_tick_drains_every_due_entry() {
        let mut schedule = RebroadcastSchedule::<4>::new();
        schedule.schedule(dest(1), InstantMillis(100));
        schedule.schedule(dest(2), InstantMillis(100));
        schedule.schedule(dest(3), InstantMillis(100));

        let mut drained = std::vec::Vec::new();
        while let Some(entry) = schedule.take_due(InstantMillis(100)) {
            drained.push(entry.destination);
        }
        drained.sort_by_key(|d| *d.as_bytes());
        assert_eq!(drained, std::vec![dest(1), dest(2), dest(3)]);
        assert_eq!(schedule.pending_count(), 0);
    }
}
