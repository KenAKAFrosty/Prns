use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::schedule::{EchoOutcome, ScheduledAnnounce, ScheduledAnnounceQueue};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapScheduledAnnounceQueue {
    destination: Vec<DestinationHash>,
    due_at: Vec<InstantMillis>,
    source_interface: Vec<InterfaceId>,
    hops: Vec<u8>,
    emission_count: Vec<u8>,
    peer_rebroadcast_count: Vec<u8>,
    directed_to: Vec<Option<InterfaceId>>,
}

impl HeapScheduledAnnounceQueue {
    fn row(&self, i: usize) -> ScheduledAnnounce {
        ScheduledAnnounce {
            destination: self.destination[i],
            due_at: self.due_at[i],
            source_interface: self.source_interface[i],
            hops: self.hops[i],
            emission_count: self.emission_count[i],
            peer_rebroadcast_count: self.peer_rebroadcast_count[i],
            directed_to: self.directed_to[i],
        }
    }

    fn push_row(&mut self, entry: ScheduledAnnounce) {
        self.destination.push(entry.destination);
        self.due_at.push(entry.due_at);
        self.source_interface.push(entry.source_interface);
        self.hops.push(entry.hops);
        self.emission_count.push(entry.emission_count);
        self.peer_rebroadcast_count
            .push(entry.peer_rebroadcast_count);
        self.directed_to.push(entry.directed_to);
    }

    fn swap_remove_row(&mut self, i: usize) {
        self.destination.swap_remove(i);
        self.due_at.swap_remove(i);
        self.source_interface.swap_remove(i);
        self.hops.swap_remove(i);
        self.emission_count.swap_remove(i);
        self.peer_rebroadcast_count.swap_remove(i);
        self.directed_to.swap_remove(i);
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
        directed_to: Option<InterfaceId>,
    ) {
        if let Some(i) = self
            .destination
            .iter()
            .position(|existing| *existing == destination)
        {
            self.due_at[i] = due_at;
            self.source_interface[i] = source_interface;
            self.hops[i] = hops;
            self.emission_count[i] = 0;
            self.peer_rebroadcast_count[i] = 0;
            self.directed_to[i] = directed_to;
        } else {
            self.push_row(ScheduledAnnounce {
                destination,
                due_at,
                source_interface,
                hops,
                emission_count: 0,
                peer_rebroadcast_count: 0,
                directed_to,
            });
        }
    }
}

impl ScheduledAnnounceQueue for HeapScheduledAnnounceQueue {
    fn scheduled_count(&self) -> usize {
        self.due_at.len()
    }
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) {
        self.upsert(destination, due_at, source_interface, hops, None);
    }
    fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) {
        self.upsert(destination, due_at, target, hops, Some(target));
    }
    fn drain_due(&mut self, now: InstantMillis) -> usize {
        let mut removed = 0;
        let mut i = 0;
        while i < self.due_at.len() {
            if self.due_at[i] <= now {
                self.swap_remove_row(i);
                removed += 1;
            } else {
                i += 1;
            }
        }
        removed
    }
    fn advance_due_retransmits(
        &mut self,
        now: InstantMillis,
        interval_ms: u64,
        max_emission_count: u8,
    ) -> usize {
        let mut completed = 0;
        let mut i = 0;
        while i < self.due_at.len() {
            if self.due_at[i].0 <= now.0 {
                let count = self.emission_count[i].saturating_add(1);
                self.emission_count[i] = count;
                if count >= max_emission_count {
                    self.swap_remove_row(i);
                    completed += 1;
                    continue;
                }
                self.due_at[i] = InstantMillis(now.0.saturating_add(interval_ms));
            }
            i += 1;
        }
        completed
    }
    fn absorb_echo(
        &mut self,
        destination: &DestinationHash,
        received_hops: u8,
        now: InstantMillis,
        max_peer_rebroadcast_count: u8,
    ) -> EchoOutcome {
        let Some(i) = self
            .destination
            .iter()
            .position(|existing| *existing == *destination)
        else {
            return EchoOutcome::NoPendingEntry;
        };
        let hops_below = received_hops.saturating_sub(1);
        let entry_hops = self.hops[i];
        let emitted = self.emission_count[i] > 0;
        if hops_below == entry_hops {
            let peers = self.peer_rebroadcast_count[i].saturating_add(1);
            self.peer_rebroadcast_count[i] = peers;
            if emitted && peers >= max_peer_rebroadcast_count {
                self.swap_remove_row(i);
                return EchoOutcome::RetransmitCancelled;
            }
            return EchoOutcome::PeerRebroadcastCounted;
        }
        if hops_below == entry_hops.saturating_add(1) && emitted && now.0 < self.due_at[i].0 {
            self.swap_remove_row(i);
            return EchoOutcome::RetransmitCancelled;
        }
        EchoOutcome::HopsUnrelated
    }
    fn earliest_due_at(&self) -> Option<InstantMillis> {
        self.due_at.iter().copied().min()
    }
    fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_ {
        (0..self.scheduled_count()).map(move |i| self.row(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn grows_past_a_fixed_cap_upserts_and_drains() {
        let mut pending = HeapScheduledAnnounceQueue::default();
        for n in 0..200u8 {
            pending.schedule(dest(n), InstantMillis(100 + n as u64), iface(0xAA), 1);
        }
        assert_eq!(pending.scheduled_count(), 200);
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(100)));

        pending.schedule(dest(0), InstantMillis(50), iface(0xBB), 1);
        assert_eq!(pending.scheduled_count(), 200);
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(50)));

        assert_eq!(pending.drain_due(InstantMillis(10_000)), 200);
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn drain_due_uses_the_due_boundary_and_reports_removed_count() {
        let mut pending = HeapScheduledAnnounceQueue::default();
        pending.schedule(dest(1), InstantMillis(99), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(3), InstantMillis(101), iface(0xAA), 1);

        assert_eq!(pending.drain_due(InstantMillis(100)), 2);
        assert_eq!(pending.scheduled_count(), 1);
        assert_eq!(pending.iter().next().unwrap().destination, dest(3));
    }

    #[test]
    fn advance_re_arms_then_completes_at_the_emission_cap() {
        let mut pending = HeapScheduledAnnounceQueue::default();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(100), 5_500, 2),
            0
        );
        let entry = pending.iter().next().unwrap();
        assert_eq!(entry.emission_count, 1);
        assert_eq!(entry.due_at, InstantMillis(5_600));

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(5_600), 5_500, 2),
            1
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn absorb_echo_counts_then_cancels_like_the_fixed_queue() {
        let mut pending = HeapScheduledAnnounceQueue::default();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);
        pending.advance_due_retransmits(InstantMillis(100), 5_500, 2);

        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(200), 2),
            EchoOutcome::PeerRebroadcastCounted
        );
        assert_eq!(
            pending.absorb_echo(&dest(1), 7, InstantMillis(300), 2),
            EchoOutcome::RetransmitCancelled
        );
        assert_eq!(pending.scheduled_count(), 0);
    }
}
