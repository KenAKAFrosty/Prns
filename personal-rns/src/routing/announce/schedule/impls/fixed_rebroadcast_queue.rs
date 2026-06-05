use heapless::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::schedule::{RebroadcastQueue, ScheduledRebroadcast};
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixedRebroadcastQueue<const MAX_PENDING: usize> {
    pending: Vec<ScheduledRebroadcast, MAX_PENDING>,
}

impl<const MAX_PENDING: usize> FixedRebroadcastQueue<MAX_PENDING> {
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
            existing.due_at = due_at;
            existing.source_interface = source_interface;
        } else {
            let _ = self.pending.push(ScheduledRebroadcast {
                destination,
                due_at,
                source_interface,
            });
        }
    }

    pub fn take_due(&mut self, now: InstantMillis) -> Option<ScheduledRebroadcast> {
        let due = self.pending.iter().position(|entry| entry.due_at <= now)?;
        Some(self.pending.swap_remove(due))
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScheduledRebroadcast> + '_ {
        self.pending.iter()
    }

    pub fn earliest_due_at(&self) -> Option<InstantMillis> {
        self.pending.iter().map(|entry| entry.due_at).min()
    }

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

    pub fn count_due(&self, now: InstantMillis) -> usize {
        self.pending
            .iter()
            .filter(|entry| entry.due_at <= now)
            .count()
    }
}

impl<const MAX_PENDING: usize> RebroadcastQueue for FixedRebroadcastQueue<MAX_PENDING> {
    fn pending_count(&self) -> usize {
        FixedRebroadcastQueue::pending_count(self)
    }
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
    ) {
        FixedRebroadcastQueue::schedule(self, destination, due_at, source_interface)
    }
    fn drain_due(&mut self, now: InstantMillis) -> usize {
        FixedRebroadcastQueue::drain_due(self, now)
    }
    fn earliest_due_at(&self) -> Option<InstantMillis> {
        FixedRebroadcastQueue::earliest_due_at(self)
    }
    fn as_slice(&self) -> &[ScheduledRebroadcast] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn nothing_is_due_before_its_time_then_it_drains_once() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));

        assert_eq!(pending.take_due(InstantMillis(99)), None);
        assert_eq!(
            pending.take_due(InstantMillis(100)),
            Some(ScheduledRebroadcast {
                destination: dest(1),
                due_at: InstantMillis(100),
                source_interface: iface(0xAA),
            })
        );
        assert_eq!(pending.take_due(InstantMillis(100)), None);
        assert_eq!(pending.pending_count(), 0);
    }

    #[test]
    fn rescheduling_a_destination_updates_time_and_source_without_duplicating() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(1), InstantMillis(200), iface(0xBB));
        assert_eq!(pending.pending_count(), 1);

        assert_eq!(pending.take_due(InstantMillis(150)), None);
        let taken = pending.take_due(InstantMillis(200)).unwrap();
        assert_eq!(taken.destination, dest(1));
        assert_eq!(taken.source_interface, iface(0xBB));
    }

    #[test]
    fn only_entries_whose_time_has_come_are_taken() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(300), iface(0xAA));

        assert_eq!(
            pending.take_due(InstantMillis(200)).map(|e| e.destination),
            Some(dest(1))
        );
        assert_eq!(pending.take_due(InstantMillis(200)), None);
        assert_eq!(
            pending.take_due(InstantMillis(300)).map(|e| e.destination),
            Some(dest(2))
        );
    }

    #[test]
    fn iter_exposes_pending_entries() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(300), iface(0xBB));

        let destinations: std::vec::Vec<_> =
            pending.iter().map(|entry| entry.destination).collect();
        assert_eq!(destinations, std::vec![dest(1), dest(2)]);
    }

    #[test]
    fn count_due_uses_the_due_boundary() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(99), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(3), InstantMillis(101), iface(0xAA));

        assert_eq!(pending.count_due(InstantMillis(100)), 2);
    }

    #[test]
    fn drain_due_returns_removed_count_and_keeps_future_entries() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(99), iface(0xAA));
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA));
        pending.schedule(dest(3), InstantMillis(101), iface(0xAA));

        assert_eq!(pending.drain_due(InstantMillis(100)), 2);
        assert_eq!(pending.pending_count(), 1);
        assert_eq!(
            pending.iter().next().map(|entry| entry.destination),
            Some(dest(3))
        );
    }

    #[test]
    fn a_tick_drains_every_due_entry() {
        let mut pending = FixedRebroadcastQueue::<4>::new();
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
