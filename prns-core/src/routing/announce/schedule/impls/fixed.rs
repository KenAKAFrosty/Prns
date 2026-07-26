use heapless::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::defaults::{
    ANNOUNCE_ONE_SHOT_INITIAL_EMISSION_COUNT, ANNOUNCE_WITH_RETRY_INITIAL_EMISSION_COUNT,
};
use crate::routing::announce::schedule::{
    EchoOutcome, ScheduleCancellation, ScheduleOutcome, ScheduleRejection, ScheduledAnnounce,
    ScheduledAnnounceQueue,
};
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixedScheduledAnnounceQueue<const MAX_PENDING: usize> {
    destination: Vec<DestinationHash, MAX_PENDING>,
    due_at: Vec<InstantMillis, MAX_PENDING>,
    source_interface: Vec<InterfaceId, MAX_PENDING>,
    hops: Vec<u8, MAX_PENDING>,
    our_emission_count: Vec<u8, MAX_PENDING>,
    peer_emission_count: Vec<u8, MAX_PENDING>,
    directed_to: Vec<Option<InterfaceId>, MAX_PENDING>,
    held: Vec<ScheduledAnnounce, MAX_PENDING>,
    earliest_due: Option<InstantMillis>,
}

impl<const MAX_PENDING: usize> FixedScheduledAnnounceQueue<MAX_PENDING> {
    pub const fn new() -> Self {
        Self {
            destination: Vec::new(),
            due_at: Vec::new(),
            source_interface: Vec::new(),
            hops: Vec::new(),
            our_emission_count: Vec::new(),
            peer_emission_count: Vec::new(),
            directed_to: Vec::new(),
            held: Vec::new(),
            earliest_due: None,
        }
    }

    fn row(&self, i: usize) -> ScheduledAnnounce {
        ScheduledAnnounce {
            destination: self.destination[i],
            due_at: self.due_at[i],
            source_interface: self.source_interface[i],
            hops: self.hops[i],
            our_emission_count: self.our_emission_count[i],
            peer_emission_count: self.peer_emission_count[i],
            directed_to: self.directed_to[i],
        }
    }

    fn push_row(&mut self, entry: ScheduledAnnounce) -> Result<(), ScheduleRejection> {
        if self.destination.len() >= MAX_PENDING {
            return Err(ScheduleRejection::QueueFull);
        }
        let _ = self.destination.push(entry.destination);
        let _ = self.due_at.push(entry.due_at);
        let _ = self.source_interface.push(entry.source_interface);
        let _ = self.hops.push(entry.hops);
        let _ = self.our_emission_count.push(entry.our_emission_count);
        let _ = self.peer_emission_count.push(entry.peer_emission_count);
        let _ = self.directed_to.push(entry.directed_to);
        Ok(())
    }

    fn swap_remove_row(&mut self, i: usize) {
        self.destination.swap_remove(i);
        self.due_at.swap_remove(i);
        self.source_interface.swap_remove(i);
        self.hops.swap_remove(i);
        self.our_emission_count.swap_remove(i);
        self.peer_emission_count.swap_remove(i);
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
        our_emission_count: u8,
    ) -> ScheduleOutcome {
        if let Some(i) = self
            .destination
            .iter()
            .position(|existing| *existing == destination)
        {
            self.due_at[i] = due_at;
            self.source_interface[i] = source_interface;
            self.hops[i] = hops;
            self.our_emission_count[i] = our_emission_count;
            self.peer_emission_count[i] = 0;
            self.directed_to[i] = directed_to;
            self.refresh_earliest();
            ScheduleOutcome::Updated
        } else {
            let outcome = match self.push_row(ScheduledAnnounce {
                destination,
                due_at,
                source_interface,
                hops,
                our_emission_count,
                peer_emission_count: 0,
                directed_to,
            }) {
                Ok(()) => ScheduleOutcome::Inserted,
                Err(rejection) => ScheduleOutcome::Rejected(rejection),
            };
            self.refresh_earliest();
            outcome
        }
    }

    fn refresh_earliest(&mut self) {
        self.earliest_due = self.due_at.iter().copied().min();
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
        let _ = self.held.push(flood);
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
        let mut still_held = Vec::new();
        while let Some(flood) = self.held.pop() {
            let destination = flood.destination;
            if self.active_directed_index(destination).is_none() {
                let _ = self.push_row(flood);
            } else {
                let _ = still_held.push(flood);
            }
        }
        still_held.as_mut_slice().reverse();
        self.held = still_held;
    }

    pub fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        self.clear_held(destination);
        self.upsert(
            destination,
            due_at,
            source_interface,
            hops,
            None,
            ANNOUNCE_WITH_RETRY_INITIAL_EMISSION_COUNT,
        )
    }

    pub fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        let replacing = self.destination.contains(&destination);
        self.park_displaced_flood(destination);
        let outcome = self.upsert(
            destination,
            due_at,
            target,
            hops,
            Some(target),
            ANNOUNCE_WITH_RETRY_INITIAL_EMISSION_COUNT,
        );
        if replacing && outcome == ScheduleOutcome::Inserted {
            ScheduleOutcome::Updated
        } else {
            outcome
        }
    }

    pub fn cancel(&mut self, destination: &DestinationHash) -> ScheduleCancellation {
        let active_removed = if let Some(index) = self
            .destination
            .iter()
            .position(|existing| existing == destination)
        {
            self.swap_remove_row(index);
            true
        } else {
            false
        };
        let parked_removed = if let Some(index) = self
            .held
            .iter()
            .position(|entry| entry.destination == *destination)
        {
            self.held.swap_remove(index);
            true
        } else {
            false
        };
        if active_removed {
            self.refresh_earliest();
        }
        ScheduleCancellation {
            active_removed,
            parked_removed,
        }
    }

    pub fn take_due(&mut self, now: InstantMillis) -> Option<ScheduledAnnounce> {
        let i = self.due_at.iter().position(|due| *due <= now)?;
        let row = self.row(i);
        self.swap_remove_row(i);
        self.refresh_earliest();
        Some(row)
    }

    pub fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_ {
        (0..self.scheduled_count()).map(move |i| self.row(i))
    }

    pub fn earliest_due_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_due,
            self.due_at.iter().copied().min(),
            "earliest_due cache desynced from due_at column"
        );
        self.earliest_due
    }

    pub fn drain_due(&mut self, now: InstantMillis) -> usize {
        let mut removed = 0;
        for i in (0..self.due_at.len()).rev() {
            if self.due_at[i] <= now {
                let before = self.due_at.len();
                self.swap_remove_row(i);
                if self.due_at.len() < before {
                    removed += 1;
                }
            }
        }
        self.refresh_earliest();
        removed
    }

    pub fn count_due(&self, now: InstantMillis) -> usize {
        self.due_at.iter().filter(|due| **due <= now).count()
    }
}

impl<const MAX_PENDING: usize> ScheduledAnnounceQueue for FixedScheduledAnnounceQueue<MAX_PENDING> {
    fn scheduled_count(&self) -> usize {
        FixedScheduledAnnounceQueue::scheduled_count(self)
    }
    fn schedule(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        FixedScheduledAnnounceQueue::schedule(self, destination, due_at, source_interface, hops)
    }
    fn schedule_directed(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        FixedScheduledAnnounceQueue::schedule_directed(self, destination, due_at, target, hops)
    }
    fn schedule_shared_client(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        source_interface: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        self.clear_held(destination);
        self.upsert(
            destination,
            due_at,
            source_interface,
            hops,
            None,
            ANNOUNCE_ONE_SHOT_INITIAL_EMISSION_COUNT,
        )
    }
    fn schedule_directed_shared_client(
        &mut self,
        destination: DestinationHash,
        due_at: InstantMillis,
        target: InterfaceId,
        hops: u8,
    ) -> ScheduleOutcome {
        let replacing = self.destination.contains(&destination);
        self.park_displaced_flood(destination);
        let outcome = self.upsert(
            destination,
            due_at,
            target,
            hops,
            Some(target),
            ANNOUNCE_ONE_SHOT_INITIAL_EMISSION_COUNT,
        );
        if replacing && outcome == ScheduleOutcome::Inserted {
            ScheduleOutcome::Updated
        } else {
            outcome
        }
    }
    fn cancel(&mut self, destination: &DestinationHash) -> ScheduleCancellation {
        FixedScheduledAnnounceQueue::cancel(self, destination)
    }
    fn drain_due(&mut self, now: InstantMillis) -> usize {
        FixedScheduledAnnounceQueue::drain_due(self, now)
    }
    fn advance_due_retransmits(
        &mut self,
        now: InstantMillis,
        interval_ms: u64,
        max_our_emission_count: u8,
    ) -> usize {
        let mut completed = 0;
        for i in (0..self.due_at.len()).rev() {
            if self.due_at[i].0 <= now.0 {
                if self.directed_to[i].is_some() && self.held_contains(self.destination[i]) {
                    let before = self.due_at.len();
                    self.swap_remove_row(i);
                    if self.due_at.len() < before {
                        completed += 1;
                        continue;
                    }
                }
                let count = self.our_emission_count[i].saturating_add(1);
                self.our_emission_count[i] = count;
                if count >= max_our_emission_count {
                    let before = self.due_at.len();
                    self.swap_remove_row(i);
                    if self.due_at.len() < before {
                        completed += 1;
                        continue;
                    }
                }
                self.due_at[i] = InstantMillis(now.0.saturating_add(interval_ms));
            }
        }
        self.restore_orphaned_held();
        self.refresh_earliest();
        completed
    }
    fn absorb_echo(
        &mut self,
        destination: &DestinationHash,
        received_hops: u8,
        now: InstantMillis,
        max_peer_emission_count: u8,
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
        let emitted = self.our_emission_count[i] > 0;
        if hops_below == entry_hops {
            let peers = self.peer_emission_count[i].saturating_add(1);
            self.peer_emission_count[i] = peers;
            if emitted && peers >= max_peer_emission_count {
                self.swap_remove_row(i);
                self.refresh_earliest();
                return EchoOutcome::RetransmitCancelled;
            }
            return EchoOutcome::PeerEmissionCounted;
        }
        if hops_below == entry_hops.saturating_add(1) && emitted && now.0 < self.due_at[i].0 {
            self.swap_remove_row(i);
            self.refresh_earliest();
            return EchoOutcome::RetransmitCancelled;
        }
        EchoOutcome::HopsUnrelated
    }
    fn earliest_due_at(&self) -> Option<InstantMillis> {
        FixedScheduledAnnounceQueue::earliest_due_at(self)
    }
    fn iter(&self) -> impl Iterator<Item = ScheduledAnnounce> + '_ {
        FixedScheduledAnnounceQueue::iter(self)
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_must_use)]

    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn fixed_capacity_reports_insert_update_and_rejection_without_eviction() {
        let mut pending = FixedScheduledAnnounceQueue::<2>::new();
        assert_eq!(
            pending.schedule(dest(1), InstantMillis(300), iface(0xAA), 9),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule(dest(1), InstantMillis(200), iface(0xBB), 1),
            ScheduleOutcome::Updated,
        );
        assert_eq!(
            pending.schedule(dest(2), InstantMillis(400), iface(0xCC), 2),
            ScheduleOutcome::Inserted,
        );
        let before = pending.iter().collect::<std::vec::Vec<_>>();

        assert_eq!(
            pending.schedule(dest(3), InstantMillis(100), iface(0xDD), 0),
            ScheduleOutcome::Rejected(ScheduleRejection::QueueFull),
        );
        assert_eq!(pending.iter().collect::<std::vec::Vec<_>>(), before);
    }

    #[test]
    fn cancellation_removes_active_and_parked_work_and_recalculates_earliest() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        assert_eq!(
            pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 2),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule(dest(2), InstantMillis(200), iface(0xAA), 2),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule_directed(dest(1), InstantMillis(50), iface(0xBB), 2),
            ScheduleOutcome::Updated,
        );
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(50)));

        assert_eq!(
            pending.cancel(&dest(1)),
            ScheduleCancellation {
                active_removed: true,
                parked_removed: true,
            },
        );
        assert_eq!(pending.earliest_due_at(), Some(InstantMillis(200)));
        assert_eq!(
            pending
                .iter()
                .map(|entry| entry.destination)
                .collect::<std::vec::Vec<_>>(),
            std::vec![dest(2)],
        );
        assert_eq!(pending.cancel(&dest(1)), ScheduleCancellation::NOT_FOUND);
    }

    #[test]
    fn directed_scheduling_reports_insertions_and_flood_replacements() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        assert_eq!(
            pending.schedule_directed(dest(1), InstantMillis(100), iface(0xB1), 1),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule(dest(2), InstantMillis(200), iface(0xA2), 2),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule_directed(dest(2), InstantMillis(250), iface(0xB2), 2),
            ScheduleOutcome::Updated,
        );
    }

    #[test]
    fn shared_client_directed_scheduling_reports_insertions_and_flood_replacements() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        assert_eq!(
            pending.schedule_directed_shared_client(dest(1), InstantMillis(100), iface(0xB1), 1,),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule_shared_client(dest(2), InstantMillis(200), iface(0xA2), 2),
            ScheduleOutcome::Inserted,
        );
        assert_eq!(
            pending.schedule_directed_shared_client(dest(2), InstantMillis(250), iface(0xB2), 2,),
            ScheduleOutcome::Updated,
        );
    }

    #[test]
    fn nothing_is_due_before_its_time_then_it_drains_once() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);

        assert_eq!(pending.take_due(InstantMillis(99)), None);
        assert_eq!(
            pending.take_due(InstantMillis(100)),
            Some(ScheduledAnnounce {
                destination: dest(1),
                due_at: InstantMillis(100),
                source_interface: iface(0xAA),
                hops: 1,
                our_emission_count: 0,
                peer_emission_count: 0,
                directed_to: None,
            })
        );
        assert_eq!(pending.take_due(InstantMillis(100)), None);
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn rescheduling_a_destination_updates_time_and_source_without_duplicating() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(1), InstantMillis(200), iface(0xBB), 1);
        assert_eq!(pending.scheduled_count(), 1);

        assert_eq!(pending.take_due(InstantMillis(150)), None);
        let taken = pending.take_due(InstantMillis(200)).unwrap();
        assert_eq!(taken.destination, dest(1));
        assert_eq!(taken.source_interface, iface(0xBB));
    }

    #[test]
    fn only_entries_whose_time_has_come_are_taken() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(300), iface(0xAA), 1);

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
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(300), iface(0xBB), 1);

        let destinations: std::vec::Vec<_> =
            pending.iter().map(|entry| entry.destination).collect();
        assert_eq!(destinations, std::vec![dest(1), dest(2)]);
    }

    #[test]
    fn count_due_uses_the_due_boundary() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(99), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(3), InstantMillis(101), iface(0xAA), 1);

        assert_eq!(pending.count_due(InstantMillis(100)), 2);
    }

    #[test]
    fn drain_due_returns_removed_count_and_keeps_future_entries() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
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
    fn a_tick_drains_every_due_entry() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(2), InstantMillis(100), iface(0xAA), 1);
        pending.schedule(dest(3), InstantMillis(100), iface(0xAA), 1);

        let mut drained = std::vec::Vec::new();
        while let Some(entry) = pending.take_due(InstantMillis(100)) {
            drained.push(entry.destination);
        }
        drained.sort_by_key(|d| *d.as_bytes());
        assert_eq!(drained, std::vec![dest(1), dest(2), dest(3)]);
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn advance_re_arms_a_due_entry_until_the_emission_cap() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(100), 5_500, 2),
            0
        );
        let entry = pending.iter().next().unwrap();
        assert_eq!(entry.our_emission_count, 1);
        assert_eq!(entry.due_at, InstantMillis(5_600));

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(5_600), 5_500, 2),
            1
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn advance_leaves_entries_that_are_not_yet_due() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(500), iface(0xAA), 5);

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(100), 5_500, 2),
            0
        );
        assert_eq!(pending.iter().next().unwrap().our_emission_count, 0);
    }

    #[test]
    fn absorb_echo_with_no_pending_entry_is_a_no_op() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(100), 2),
            EchoOutcome::NoPendingEntry
        );
    }

    #[test]
    fn a_same_distance_echo_only_counts_until_we_have_emitted() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);

        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(150), 2),
            EchoOutcome::PeerEmissionCounted
        );
        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(160), 2),
            EchoOutcome::PeerEmissionCounted
        );
        assert_eq!(pending.scheduled_count(), 1);
        assert_eq!(pending.iter().next().unwrap().peer_emission_count, 2);
    }

    #[test]
    fn a_same_distance_echo_cancels_after_we_emit_and_reach_the_cap() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);
        pending.advance_due_retransmits(InstantMillis(100), 5_500, 2);

        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(200), 2),
            EchoOutcome::PeerEmissionCounted
        );
        assert_eq!(
            pending.absorb_echo(&dest(1), 6, InstantMillis(300), 2),
            EchoOutcome::RetransmitCancelled
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn an_onward_echo_cancels_the_pending_retransmit_before_its_deadline() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);
        pending.advance_due_retransmits(InstantMillis(100), 5_500, 2);

        assert_eq!(
            pending.absorb_echo(&dest(1), 7, InstantMillis(200), 2),
            EchoOutcome::RetransmitCancelled
        );
        assert_eq!(pending.scheduled_count(), 0);
    }

    #[test]
    fn an_onward_echo_at_or_after_the_deadline_leaves_the_entry() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);
        pending.advance_due_retransmits(InstantMillis(100), 5_500, 2);

        assert_eq!(
            pending.absorb_echo(&dest(1), 7, InstantMillis(5_600), 2),
            EchoOutcome::HopsUnrelated
        );
        assert_eq!(pending.scheduled_count(), 1);
    }

    #[test]
    fn an_unrelated_hop_count_touches_nothing() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 5);

        assert_eq!(
            pending.absorb_echo(&dest(1), 10, InstantMillis(150), 2),
            EchoOutcome::HopsUnrelated
        );
        assert_eq!(pending.iter().next().unwrap().peer_emission_count, 0);
    }

    #[test]
    fn a_directed_answer_parks_a_displaced_flood_and_restores_it_after_it_fires() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 2);
        pending.schedule_directed(dest(1), InstantMillis(200), iface(0xBB), 2);

        assert_eq!(pending.held_count(), 1, "the displaced flood is parked");
        assert_eq!(pending.scheduled_count(), 1);
        let active = pending.iter().next().unwrap();
        assert_eq!(active.directed_to, Some(iface(0xBB)));
        assert_eq!(active.due_at, InstantMillis(200));

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(200), 5_500, 2),
            1,
            "the directed answer is one-shot and retires on its first emission",
        );
        assert_eq!(pending.held_count(), 0);
        assert_eq!(pending.scheduled_count(), 1);
        let restored = pending.iter().next().unwrap();
        assert_eq!(
            restored.directed_to, None,
            "the parked flood is back, untouched"
        );
        assert_eq!(restored.source_interface, iface(0xAA));
        assert_eq!(restored.due_at, InstantMillis(100));
    }

    #[test]
    fn restoring_orphaned_floods_keeps_other_directed_floods_parked() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xA1), 1);
        pending.schedule(dest(2), InstantMillis(200), iface(0xA2), 2);
        pending.schedule_directed(dest(1), InstantMillis(100), iface(0xB1), 1);
        pending.schedule_directed(dest(2), InstantMillis(200), iface(0xB2), 2);
        assert_eq!(pending.held_count(), 2);

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(100), 50, 3),
            1,
        );
        assert_eq!(pending.held_count(), 1);
        let restored = pending
            .iter()
            .find(|entry| entry.destination == dest(1))
            .unwrap();
        assert_eq!(restored.directed_to, None);
        let directed = pending
            .iter()
            .find(|entry| entry.destination == dest(2))
            .unwrap();
        assert_eq!(directed.directed_to, Some(iface(0xB2)));
    }

    #[test]
    fn a_fresher_flood_clears_the_parked_held_entry() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule(dest(1), InstantMillis(100), iface(0xAA), 2);
        pending.schedule_directed(dest(1), InstantMillis(200), iface(0xBB), 2);
        assert_eq!(pending.held_count(), 1);

        pending.schedule(dest(1), InstantMillis(300), iface(0xCC), 3);
        assert_eq!(
            pending.held_count(),
            0,
            "a fresher announce supersedes and drops the parked flood",
        );
        assert_eq!(pending.scheduled_count(), 1);
        let active = pending.iter().next().unwrap();
        assert_eq!(active.directed_to, None);
        assert_eq!(active.due_at, InstantMillis(300));
    }

    #[test]
    fn a_directed_answer_with_no_displaced_flood_retries_like_a_normal_entry() {
        let mut pending = FixedScheduledAnnounceQueue::<4>::new();
        pending.schedule_directed(dest(1), InstantMillis(100), iface(0xBB), 2);
        assert_eq!(pending.held_count(), 0);

        assert_eq!(
            pending.advance_due_retransmits(InstantMillis(100), 5_500, 2),
            0,
            "with nothing parked the directed answer re-arms instead of being one-shot",
        );
        let entry = pending.iter().next().unwrap();
        assert_eq!(entry.our_emission_count, 1);
        assert_eq!(entry.due_at, InstantMillis(5_600));
    }
}
