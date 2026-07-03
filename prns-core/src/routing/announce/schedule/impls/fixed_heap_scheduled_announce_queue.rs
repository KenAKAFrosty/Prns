//! The fixed-capacity, heap-backed twin of [`FixedScheduledAnnounceQueue`]: the seven columns
//! live in a caller-chosen heap region (PSRAM on the S3) via `A`, each pre-reserved to `MAX_PENDING` so a push never reallocates.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::schedule::{EchoOutcome, ScheduledAnnounce, ScheduledAnnounceQueue};
use crate::wire::DestinationHash;

pub struct FixedHeapScheduledAnnounceQueue<const MAX_PENDING: usize, A: Allocator = Global> {
    destination: Vec<DestinationHash, A>,
    due_at: Vec<InstantMillis, A>,
    source_interface: Vec<InterfaceId, A>,
    hops: Vec<u8, A>,
    emission_count: Vec<u8, A>,
    peer_rebroadcast_count: Vec<u8, A>,
    directed_to: Vec<Option<InterfaceId>, A>,
    held: Vec<ScheduledAnnounce, A>,
}

impl<const MAX_PENDING: usize, A: Allocator + Default> Default
    for FixedHeapScheduledAnnounceQueue<MAX_PENDING, A>
{
    fn default() -> Self {
        Self {
            destination: Vec::with_capacity_in(MAX_PENDING, A::default()),
            due_at: Vec::with_capacity_in(MAX_PENDING, A::default()),
            source_interface: Vec::with_capacity_in(MAX_PENDING, A::default()),
            hops: Vec::with_capacity_in(MAX_PENDING, A::default()),
            emission_count: Vec::with_capacity_in(MAX_PENDING, A::default()),
            peer_rebroadcast_count: Vec::with_capacity_in(MAX_PENDING, A::default()),
            directed_to: Vec::with_capacity_in(MAX_PENDING, A::default()),
            held: Vec::with_capacity_in(MAX_PENDING, A::default()),
        }
    }
}

impl<const MAX_PENDING: usize, A: Allocator + Default>
    FixedHeapScheduledAnnounceQueue<MAX_PENDING, A>
{
    pub fn new() -> Self {
        Self::default()
    }
}

impl<const MAX_PENDING: usize, A: Allocator> FixedHeapScheduledAnnounceQueue<MAX_PENDING, A> {
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
        if self.destination.len() >= MAX_PENDING {
            return;
        }
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

    pub fn scheduled_count(&self) -> usize {
        self.due_at.len()
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

    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    fn held_contains(&self, destination: DestinationHash) -> bool {
        self.held
            .iter()
            .any(|entry| entry.destination == destination)
    }

    fn active_directed_index(&self, destination: DestinationHash) -> Option<usize> {
        self.destination
            .iter()
            .zip(self.directed_to.iter())
            .position(|(dst, directed)| *dst == destination && directed.is_some())
    }

    fn park_displaced_flood(&mut self, destination: DestinationHash) {
        let Some(i) = self
            .destination
            .iter()
            .position(|existing| *existing == destination)
        else {
            return;
        };
        if self.directed_to[i].is_some() {
            return;
        }
        let flood = self.row(i);
        self.swap_remove_row(i);
        if self.held.len() < MAX_PENDING {
            self.held.push(flood);
        }
    }

    fn clear_held(&mut self, destination: DestinationHash) {
        if let Some(j) = self
            .held
            .iter()
            .position(|entry| entry.destination == destination)
        {
            self.held.swap_remove(j);
        }
    }

    fn restore_orphaned_held(&mut self) {
        let mut j = 0;
        while j < self.held.len() {
            let destination = self.held[j].destination;
            if self.active_directed_index(destination).is_none() {
                let flood = self.held.swap_remove(j);
                self.push_row(flood);
            } else {
                j += 1;
            }
        }
    }

    pub fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) {
        self.clear_held(destination);
        self.upsert(destination, due_at, source_interface, hops, None);
    }

    pub fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) {
        self.park_displaced_flood(destination);
        self.upsert(destination, due_at, target, hops, Some(target));
    }

    pub fn take_due(&mut self, now: InstantMillis) -> Option<ScheduledAnnounce> {
        let i = self.due_at.iter().position(|due| *due <= now)?;
        let row = self.row(i);
        self.swap_remove_row(i);
        Some(row)
    }

    pub fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_ {
        (0..self.scheduled_count()).map(move |i| self.row(i))
    }

    pub fn earliest_due_at(&self) -> Option<InstantMillis> {
        self.due_at.iter().copied().min()
    }

    pub fn drain_due(&mut self, now: InstantMillis) -> usize {
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

    pub fn count_due(&self, now: InstantMillis) -> usize {
        self.due_at.iter().filter(|due| **due <= now).count()
    }
}

impl<const MAX_PENDING: usize, A: Allocator> ScheduledAnnounceQueue
    for FixedHeapScheduledAnnounceQueue<MAX_PENDING, A>
{
    fn scheduled_count(&self) -> usize {
        FixedHeapScheduledAnnounceQueue::scheduled_count(self)
    }
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) {
        FixedHeapScheduledAnnounceQueue::schedule(self, destination, due_at, source_interface, hops)
    }
    fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) {
        FixedHeapScheduledAnnounceQueue::schedule_directed(self, destination, due_at, target, hops)
    }
    fn drain_due(&mut self, now: InstantMillis) -> usize {
        FixedHeapScheduledAnnounceQueue::drain_due(self, now)
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
                if self.directed_to[i].is_some() && self.held_contains(self.destination[i]) {
                    self.swap_remove_row(i);
                    completed += 1;
                    continue;
                }
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
        self.restore_orphaned_held();
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
        FixedHeapScheduledAnnounceQueue::earliest_due_at(self)
    }
    fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_ {
        FixedHeapScheduledAnnounceQueue::iter(self)
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
    fn nothing_is_due_before_its_time_then_it_drains_once() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);

        assert_eq!(pending.take_due(InstantMillis(99)), None);
        assert_eq!(
            pending.take_due(InstantMillis(100)),
            Some(ScheduledAnnounce {
                destination: dest(1),
                due_at: InstantMillis(100),
                source_interface: iface(0xAA),
                hops: 1,
                emission_count: 0,
                peer_rebroadcast_count: 0,
                directed_to: None,
            })
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn rescheduling_a_destination_updates_without_duplicating() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(1), InstantMillis(200), iface(0xBB), 1);
        assert_eq!(pending.scheduled_count(), 1);
        let taken = pending.take_due(InstantMillis(200)).unwrap();
        assert_eq!(taken.source_interface, iface(0xBB));
    }

    #[test]
    fn a_full_queue_refuses_more_rows() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<2>::new();
        pending.schedule(dest(1), InstantMillis(1), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(1), iface(0xAA), 1);
        pending.schedule(dest(3), InstantMillis(1), iface(0xAA), 1);
        assert_eq!(pending.scheduled_count(), 2);
    }

    #[test]
    fn drain_due_returns_removed_count_and_keeps_future_entries() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(99), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(3), InstantMillis(101), iface(0xAA), 1);

        assert_eq!(pending.drain_due(InstantMillis(100)), 2);
        assert_eq!(pending.scheduled_count(), 1);
        assert_eq!(
            pending.iter().next().map(|entry| entry.destination),
            Some(dest(3))
        );
    }

    #[test]
    fn advance_re_arms_a_due_entry_until_the_emission_cap() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
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
    fn a_same_distance_echo_cancels_after_we_emit_and_reach_the_cap() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);
        pending.advance_due_retransmits(InstantMillis(100), 5_500, 2);

        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(200), 2),
            EchoOutcome::PeerRebroadcastCounted
        );
        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(300), 2),
            EchoOutcome::RetransmitCancelled
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    fn dest_n(n: u32) -> DestinationHash {
        let mut b = [0u8; 16];
        b[..4].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(b)
    }

    #[test]
    fn the_bulk_queue_carries_a_large_table() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<2048>::new();
        for n in 0..2048u32 {
            pending.schedule(dest_n(n), InstantMillis(n as u64), iface(0xAA), 1);
        }
        assert_eq!(pending.scheduled_count(), 2048);
        pending.schedule(dest_n(9999), InstantMillis(1), iface(0xAA), 1);
        assert_eq!(
            pending.scheduled_count(),
            2048,
            "a full queue refuses the overflow row"
        );
        assert_eq!(pending.drain_due(InstantMillis(u64::MAX)), 2048);
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn a_directed_answer_parks_then_restores_a_displaced_flood() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 2);
        pending.schedule_directed(dest(1), InstantMillis(200), iface(0xBB), 2);
        assert_eq!(pending.held_count(), 1);
        assert_eq!(
            pending.iter().next().unwrap().directed_to,
            Some(iface(0xBB))
        );

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(200), 5_500, 2),
            1
        );
        assert_eq!(pending.held_count(), 0);
        let restored = pending.iter().next().unwrap();
        assert_eq!(restored.directed_to, None);
        assert_eq!(restored.due_at, InstantMillis(100));
    }

    #[test]
    fn the_held_store_stays_within_the_fixed_cap() {
        let mut pending = FixedHeapScheduledAnnounceQueue::<2>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule_directed(dest(1), InstantMillis(100), iface(0xBB), 1);
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA), 1);
        pending.schedule_directed(dest(2), InstantMillis(100), iface(0xBB), 1);
        assert!(
            pending.held_count() <= 2,
            "held never exceeds the fixed cap"
        );
    }
}
