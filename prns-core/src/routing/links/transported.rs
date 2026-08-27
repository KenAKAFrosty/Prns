//! The links this node carries for others (RNS 1.4.2's `Transport.link_table`).

use crate::engine::InstantMillis;
use crate::interfaces::{BitrateBps, InterfaceId};
use crate::routing::links::{LinkId, LinkMode};
use crate::routing::routes::{RouteEvidenceHandle, RouteEvidenceId};
use crate::routing::timing::broadcast_airtime_ms;
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TransportId};

/// RNS 1.4.2 `Transport.LINK_TIMEOUT = Link.STALE_TIME × 1.25`: a switched frame refreshes the row, so only a truly dead link goes idle this long.
pub const TRANSPORTED_LINK_TIMEOUT_MS: u64 = 900_000;

/// RNS 1.5 `Transport.extra_link_proof_timeout`: one MTU's airtime on the selected
/// outbound interface, an allowance for slow remaining hops.
#[must_use]
pub fn extra_link_proof_timeout_ms(bitrate: BitrateBps) -> u64 {
    broadcast_airtime_ms(bitrate)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportedLink {
    pub link_id: LinkId,
    pub destination: DestinationHash,
    pub route_evidence: RouteEvidenceHandle,
    pub mode: LinkMode,
    pub next_hop: Option<TransportId>,
    pub next_hop_interface: InterfaceId,
    pub received_interface: InterfaceId,
    pub taken_hops: u8,
    pub remaining_hops: u8,
    pub validated_by_proof: bool,
    pub last_active: InstantMillis,
    pub proof_timeout: InstantMillis,
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<TransportedLink>() == 96);

impl crate::lemire_index::IndexRow for TransportedLink {
    type Key = LinkId;

    fn index_key(&self) -> &Self::Key {
        &self.link_id
    }
}

impl TransportedLink {
    fn deadline(&self) -> InstantMillis {
        if self.validated_by_proof {
            InstantMillis(
                self.last_active
                    .0
                    .saturating_add(TRANSPORTED_LINK_TIMEOUT_MS),
            )
        } else {
            self.proof_timeout
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSwitch {
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidateByProofError {
    UnknownLink,
    AlreadyValidated,
    WrongInterface,
    HopMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    UnknownLink,
    NotValidated,
    WrongInterface,
    HopMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackTransportedLinkError {
    AlreadyTracked,
    TableFull,
}

pub trait TransportedLinkTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> &[TransportedLink];
    fn entries_mut(&mut self) -> &mut [TransportedLink];

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.entries()
            .iter()
            .position(|entry| entry.link_id == *link_id)
    }
    fn push(&mut self, entry: TransportedLink) -> Result<(), TablePushError>;
    fn swap_remove(&mut self, index: usize);

    fn deadline_updated(&mut self, _index: usize) {}

    fn earliest_indexed_deadline(&mut self) -> Option<InstantMillis> {
        self.entries()
            .iter()
            .map(TransportedLink::deadline)
            .min_by_key(|deadline| deadline.0)
    }

    fn first_overdue(&mut self, now: InstantMillis) -> Option<usize> {
        self.entries()
            .iter()
            .position(|entry| entry.deadline().0 <= now.0)
    }
}

#[derive(Debug, Default)]
pub struct TransportedLinks<C: TransportedLinkTable> {
    table: C,
    earliest_deadline: Option<InstantMillis>,
}

impl<C: TransportedLinkTable> TransportedLinks<C> {
    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.table.index_of(link_id)
    }

    pub fn track(&mut self, entry: TransportedLink) -> Result<(), TrackTransportedLinkError> {
        if self.index_of(&entry.link_id).is_some() {
            return Err(TrackTransportedLinkError::AlreadyTracked);
        }
        let tracked = self
            .table
            .push(entry)
            .map_err(|TablePushError::TableFull| TrackTransportedLinkError::TableFull);
        self.refresh_earliest_deadline();
        tracked
    }

    pub fn entry_for(&self, link_id: &LinkId) -> Option<&TransportedLink> {
        self.index_of(link_id)
            .and_then(|index| self.table.entries().get(index))
    }

    pub(crate) fn route_evidence_ids(&self) -> impl Iterator<Item = RouteEvidenceId> + '_ {
        self.table
            .entries()
            .iter()
            .map(|entry| entry.route_evidence.id)
    }

    /// The returning LRPROOF's gate. RNS 1.4.2 transports a proof only when it arrives over the next hop with exactly the remaining hop count.
    /// The row validates and the proof leaves toward the initiator's side.
    /// Intentional deviation from reference: a second proof for a validated row is refused, where the reference re-relays it.
    pub fn validate_by_proof(
        &mut self,
        link_id: &LinkId,
        arrived_on: InterfaceId,
        received_hops: u8,
        now: InstantMillis,
    ) -> Result<TransportSwitch, ValidateByProofError> {
        let index = self
            .index_of(link_id)
            .ok_or(ValidateByProofError::UnknownLink)?;
        let entry = self
            .table
            .entries_mut()
            .get_mut(index)
            .ok_or(ValidateByProofError::UnknownLink)?;
        if entry.validated_by_proof {
            return Err(ValidateByProofError::AlreadyValidated);
        }
        if arrived_on != entry.next_hop_interface {
            return Err(ValidateByProofError::WrongInterface);
        }
        if received_hops != entry.remaining_hops {
            return Err(ValidateByProofError::HopMismatch);
        }
        entry.validated_by_proof = true;
        entry.last_active = now;
        let fire_on = entry.received_interface;
        self.table.deadline_updated(index);
        self.refresh_earliest_deadline();
        Ok(TransportSwitch { fire_on })
    }

    pub fn rebalance_and_validate_by_proof(
        &mut self,
        link_id: &LinkId,
        arrived_on: InterfaceId,
        received_hops: u8,
        now: InstantMillis,
    ) -> Result<TransportSwitch, ValidateByProofError> {
        let index = self
            .index_of(link_id)
            .ok_or(ValidateByProofError::UnknownLink)?;
        let entry = self
            .table
            .entries_mut()
            .get_mut(index)
            .ok_or(ValidateByProofError::UnknownLink)?;
        if entry.validated_by_proof {
            return Err(ValidateByProofError::AlreadyValidated);
        }
        if arrived_on != entry.next_hop_interface {
            return Err(ValidateByProofError::WrongInterface);
        }
        if received_hops == entry.remaining_hops {
            return Err(ValidateByProofError::HopMismatch);
        }
        entry.remaining_hops = received_hops;
        entry.validated_by_proof = true;
        entry.last_active = now;
        let fire_on = entry.received_interface;
        self.table.deadline_updated(index);
        self.refresh_earliest_deadline();
        Ok(TransportSwitch { fire_on })
    }

    /// RNS 1.4.2's link-table relay: a packet switches through the row toward whichever side it did not arrive from, gated on the exact hop count that side expects; one shared interface accepts either count.
    /// Intentional deviation from reference: nothing switches before the proof validates the row because no legitimate traffic can flow ahead of the proof.
    pub fn switch_through(
        &mut self,
        link_id: &LinkId,
        arrived_on: InterfaceId,
        received_hops: u8,
        now: InstantMillis,
    ) -> Result<TransportSwitch, SwitchError> {
        let index = self.index_of(link_id).ok_or(SwitchError::UnknownLink)?;
        let entry = self
            .table
            .entries_mut()
            .get_mut(index)
            .ok_or(SwitchError::UnknownLink)?;
        if !entry.validated_by_proof {
            return Err(SwitchError::NotValidated);
        }
        let fire_on = if entry.next_hop_interface == entry.received_interface {
            (received_hops == entry.remaining_hops || received_hops == entry.taken_hops)
                .then_some(entry.next_hop_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else if arrived_on == entry.next_hop_interface {
            (received_hops == entry.remaining_hops)
                .then_some(entry.received_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else if arrived_on == entry.received_interface {
            (received_hops == entry.taken_hops)
                .then_some(entry.next_hop_interface)
                .ok_or(SwitchError::HopMismatch)?
        } else {
            return Err(SwitchError::WrongInterface);
        };
        entry.last_active = now;
        self.table.deadline_updated(index);
        self.refresh_earliest_deadline();
        Ok(TransportSwitch { fire_on })
    }

    fn refresh_earliest_deadline(&mut self) {
        self.earliest_deadline = self.table.earliest_indexed_deadline();
    }

    pub fn earliest_deadline(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_deadline,
            self.table
                .entries()
                .iter()
                .map(TransportedLink::deadline)
                .min_by_key(|deadline| deadline.0),
            "earliest_deadline cache desynced from the transported-link deadlines"
        );
        self.earliest_deadline
    }

    pub fn pop_overdue(&mut self, now: InstantMillis) -> Option<TransportedLink> {
        let index = self.table.first_overdue(now)?;
        let entry = *self.table.entries().get(index)?;
        self.table.swap_remove(index);
        self.refresh_earliest_deadline();
        Some(entry)
    }

    pub fn cull_interface_orphans(
        &mut self,
        interface_present: impl Fn(InterfaceId) -> bool,
        mut on_culled: impl FnMut(InterfaceId),
    ) {
        let mut index = 0;
        while index < self.table.len() {
            let entry = self.table.entries()[index];
            if interface_present(entry.next_hop_interface)
                && interface_present(entry.received_interface)
            {
                index += 1;
            } else {
                on_culled(entry.next_hop_interface);
                on_culled(entry.received_interface);
                self.table.swap_remove(index);
            }
        }
        self.refresh_earliest_deadline();
    }

    pub fn transported_link_count_via(&self, interface: InterfaceId) -> usize {
        self.table
            .entries()
            .iter()
            .filter(|entry| {
                entry.validated_by_proof
                    && (entry.next_hop_interface == interface
                        || entry.received_interface == interface)
            })
            .count()
    }

    pub fn validated_count(&self) -> usize {
        self.table
            .entries()
            .iter()
            .filter(|entry| entry.validated_by_proof)
            .count()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestTransported = TransportedLinks<FixedTransportedLinkTable<3>>;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn entry(link: u8, validated_by_proof: bool) -> TransportedLink {
        TransportedLink {
            link_id: LinkId::new([link; 16]),
            destination: DestinationHash::new([0xDD; 16]),
            route_evidence: RouteEvidenceHandle::new(RouteEvidenceId::FIRST, 0),
            mode: LinkMode::Aes256Cbc,
            next_hop: Some(TransportId::new([0x77; 16])),
            next_hop_interface: iface(0xB2),
            received_interface: iface(0xA1),
            taken_hops: 1,
            remaining_hops: 1,
            validated_by_proof,
            last_active: InstantMillis(1_000),
            proof_timeout: InstantMillis(9_000),
        }
    }

    #[test]
    fn a_transported_link_whose_interface_left_the_view_is_culled() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, true)).unwrap();
        let mut on_gone = entry(2, true);
        on_gone.next_hop_interface = iface(0xEE);
        transported.track(on_gone).unwrap();

        transported.cull_interface_orphans(|id| id != iface(0xEE), |_| {});

        assert_eq!(transported.len(), 1);
        assert!(transported.entry_for(&LinkId::new([1; 16])).is_some());
        assert!(
            transported.entry_for(&LinkId::new([2; 16])).is_none(),
            "the row whose next hop is no longer attached is gone",
        );
    }

    #[test]
    fn the_proof_gate_validates_once_over_the_right_side_only() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();

        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xA1),
                1,
                InstantMillis(2_000)
            ),
            Err(ValidateByProofError::WrongInterface),
            "a proof from the initiator's side validates nothing",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                2,
                InstantMillis(2_000)
            ),
            Err(ValidateByProofError::HopMismatch),
            "a hop mismatch validates nothing",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                1,
                InstantMillis(2_000)
            ),
            Ok(TransportSwitch {
                fire_on: iface(0xA1),
            }),
            "the right side and hop count validate and leave toward the initiator",
        );
        assert_eq!(
            transported.validate_by_proof(
                &LinkId::new([1; 16]),
                iface(0xB2),
                1,
                InstantMillis(2_100)
            ),
            Err(ValidateByProofError::AlreadyValidated),
            "a validated row never re-validates",
        );
    }

    #[test]
    fn a_rebalanced_proof_updates_hops_and_validates_once() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();
        let link_id = LinkId::new([1; 16]);

        assert_eq!(
            transported.rebalance_and_validate_by_proof(
                &link_id,
                iface(0xB2),
                3,
                InstantMillis(2_000),
            ),
            Ok(TransportSwitch {
                fire_on: iface(0xA1),
            }),
        );
        assert_eq!(
            transported.entry_for(&link_id).copied(),
            Some(TransportedLink {
                remaining_hops: 3,
                validated_by_proof: true,
                last_active: InstantMillis(2_000),
                ..entry(1, false)
            }),
        );
        assert_eq!(
            transported.rebalance_and_validate_by_proof(
                &link_id,
                iface(0xB2),
                4,
                InstantMillis(2_100),
            ),
            Err(ValidateByProofError::AlreadyValidated),
        );
    }

    #[test]
    fn switching_obeys_direction_and_hops_and_refreshes_life() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, true)).unwrap();
        let link = LinkId::new([1; 16]);

        assert_eq!(
            transported.switch_through(&link, iface(0xA1), 1, InstantMillis(2_000)),
            Ok(TransportSwitch {
                fire_on: iface(0xB2),
            }),
            "a frame from the initiator's side leaves toward the destination",
        );
        assert_eq!(
            transported.switch_through(&link, iface(0xB2), 1, InstantMillis(2_100)),
            Ok(TransportSwitch {
                fire_on: iface(0xA1),
            }),
            "a frame from the destination's side leaves toward the initiator",
        );
        assert_eq!(
            transported.switch_through(&link, iface(0xA1), 7, InstantMillis(2_200)),
            Err(SwitchError::HopMismatch),
            "a hop mismatch repeats nothing",
        );
        assert_eq!(
            transported.switch_through(&link, iface(0xEE), 1, InstantMillis(2_300)),
            Err(SwitchError::WrongInterface),
            "an unknown interface repeats nothing",
        );

        assert_eq!(
            transported.earliest_deadline(),
            Some(InstantMillis(2_100 + TRANSPORTED_LINK_TIMEOUT_MS)),
            "every switched frame pushes the idle deadline",
        );
    }

    #[test]
    fn overdue_rows_drain_by_their_own_rule() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();
        transported.track(entry(2, true)).unwrap();

        assert_eq!(transported.pop_overdue(InstantMillis(8_999)), None);
        let popped = transported.pop_overdue(InstantMillis(9_000)).unwrap();
        assert_eq!(popped.link_id, LinkId::new([1; 16]));
        assert!(
            !popped.validated_by_proof,
            "the unvalidated row dies at proof timeout"
        );

        assert_eq!(transported.pop_overdue(InstantMillis(9_000)), None);
        let popped = transported
            .pop_overdue(InstantMillis(1_000 + TRANSPORTED_LINK_TIMEOUT_MS))
            .unwrap();
        assert_eq!(popped.link_id, LinkId::new([2; 16]));
        assert!(transported.is_empty());
    }

    #[test]
    fn duplicates_and_overflow_are_refused() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, false)).unwrap();
        assert_eq!(
            transported.track(entry(1, false)),
            Err(TrackTransportedLinkError::AlreadyTracked)
        );
        transported.track(entry(2, false)).unwrap();
        transported.track(entry(3, false)).unwrap();
        assert_eq!(
            transported.track(entry(4, false)),
            Err(TrackTransportedLinkError::TableFull)
        );
    }

    #[test]
    fn transported_link_count_via_reports_validated_carried_links_on_either_side() {
        let mut transported = TestTransported::default();
        transported.track(entry(1, true)).unwrap();
        transported.track(entry(2, false)).unwrap();
        assert_eq!(transported.validated_count(), 1);

        assert_eq!(
            transported.transported_link_count_via(iface(0xB2)),
            1,
            "the validated row counts on the side it leaves by; the unvalidated one never does",
        );
        assert_eq!(
            transported.transported_link_count_via(iface(0xA1)),
            1,
            "and on the side it arrived from, since the link rides over both",
        );
        assert_eq!(
            transported.transported_link_count_via(iface(0xEE)),
            0,
            "an interface this link never touches reads zero",
        );
    }

    #[test]
    fn the_extra_proof_allowance_is_one_mtu_of_airtime() {
        assert_eq!(extra_link_proof_timeout_ms(BitrateBps::guess(500_000)), 8);
        assert_eq!(extra_link_proof_timeout_ms(BitrateBps::guess(1_000)), 4_000);
    }

    #[test]
    fn the_heap_side_index_stays_consistent_through_track_and_reap_churn() {
        fn link_n(n: u32) -> LinkId {
            let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&key.to_be_bytes());
            b[8..12].copy_from_slice(&n.to_be_bytes());
            LinkId::new(b)
        }
        fn entry_n(n: u32) -> TransportedLink {
            let mut e = entry(0, true);
            e.link_id = link_n(n);
            e
        }

        let mut links: TransportedLinks<HeapTransportedLinkTable> = TransportedLinks::default();
        let mut live: std::vec::Vec<u32> = std::vec::Vec::new();
        let mut rng = 0x0123_4567_89AB_CDEFu64;
        let mut next = 0u32;

        for _ in 0..1_000 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let insert = live.len() < 2 || !(rng >> 33).is_multiple_of(3);
            if insert {
                let id = next;
                next += 1;
                links.track(entry_n(id)).expect("the heap table grows");
                live.push(id);
            } else {
                let popped = links
                    .pop_overdue(InstantMillis(u64::MAX))
                    .expect("a live relayed link drains through the reaper");
                live.retain(|&id| link_n(id) != popped.link_id);
            }
            for &id in &live {
                assert!(
                    links.entry_for(&link_n(id)).is_some(),
                    "every live relayed link still resolves through the index",
                );
            }
            assert!(links.entry_for(&link_n(next + 7)).is_none());
        }
        assert!(
            live.len() > 50,
            "the run must grow enough to force reindexing"
        );
    }
}

use heapless::Vec as HeaplessVec;

#[derive(Debug, Default)]
pub struct FixedTransportedLinkTable<const MAX_TRANSIT_LINKS: usize> {
    entries: HeaplessVec<TransportedLink, MAX_TRANSIT_LINKS>,
}

impl<const MAX_TRANSIT_LINKS: usize> TransportedLinkTable
    for FixedTransportedLinkTable<MAX_TRANSIT_LINKS>
{
    fn capacity(&self) -> usize {
        MAX_TRANSIT_LINKS
    }
    fn len(&self) -> usize {
        self.entries.len()
    }
    fn entries(&self) -> &[TransportedLink] {
        &self.entries
    }
    fn entries_mut(&mut self) -> &mut [TransportedLink] {
        &mut self.entries
    }
    fn push(&mut self, entry: TransportedLink) -> Result<(), TablePushError> {
        self.entries
            .push(entry)
            .map_err(|_| TablePushError::TableFull)
    }
    fn swap_remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.swap_remove(index);
        }
    }
}

/// A fixed number of transported-link rows reserved directly in a caller-selected heap.
///
/// ESP32-S3 uses this with its PSRAM allocator so relay capacity does not consume internal SRAM.
#[cfg(feature = "external-alloc")]
pub struct FixedHeapTransportedLinkTable<
    const MAX_TRANSIT_LINKS: usize,
    A: allocator_api2::alloc::Allocator = allocator_api2::alloc::Global,
> {
    entries: allocator_api2::vec::Vec<TransportedLink, A>,
}

#[cfg(feature = "external-alloc")]
impl<const MAX_TRANSIT_LINKS: usize, A: allocator_api2::alloc::Allocator + Default> Default
    for FixedHeapTransportedLinkTable<MAX_TRANSIT_LINKS, A>
{
    fn default() -> Self {
        Self {
            entries: allocator_api2::vec::Vec::with_capacity_in(MAX_TRANSIT_LINKS, A::default()),
        }
    }
}

#[cfg(feature = "external-alloc")]
impl<const MAX_TRANSIT_LINKS: usize, A: allocator_api2::alloc::Allocator> TransportedLinkTable
    for FixedHeapTransportedLinkTable<MAX_TRANSIT_LINKS, A>
{
    fn capacity(&self) -> usize {
        MAX_TRANSIT_LINKS
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn entries(&self) -> &[TransportedLink] {
        &self.entries
    }

    fn entries_mut(&mut self) -> &mut [TransportedLink] {
        &mut self.entries
    }

    fn push(&mut self, entry: TransportedLink) -> Result<(), TablePushError> {
        if self.len() >= MAX_TRANSIT_LINKS {
            return Err(TablePushError::TableFull);
        }
        self.entries.push(entry);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.swap_remove(index);
        }
    }
}

#[cfg(feature = "alloc")]
mod heap_transit_link_columns {
    use super::{TablePushError, TransportedLink, TransportedLinkTable};
    use crate::lemire_index::HeapLemireIndex;
    use crate::routing::links::LinkId;
    #[cfg(feature = "std")]
    use crate::routing::temporal_index::HeapDeadlineIndex;
    use alloc::vec::Vec;

    /// Grows with demand; how many links a relay carries is the network's business, not a storage constant. The side index is an open-addressing table keyed by the link id's leading bytes (already uniform, so a Lemire multiply-shift), probed linearly, deleted by backward-shift so a churning table never silts up.
    #[derive(Debug, Default)]
    pub struct HeapTransportedLinkTable {
        entries: Vec<TransportedLink>,
        index: HeapLemireIndex,
        #[cfg(feature = "std")]
        deadline_index: HeapDeadlineIndex,
    }

    impl TransportedLinkTable for HeapTransportedLinkTable {
        fn capacity(&self) -> usize {
            usize::MAX
        }
        fn len(&self) -> usize {
            self.entries.len()
        }
        fn entries(&self) -> &[TransportedLink] {
            &self.entries
        }
        fn entries_mut(&mut self) -> &mut [TransportedLink] {
            &mut self.entries
        }
        fn index_of(&self, link_id: &LinkId) -> Option<usize> {
            self.index.get(link_id, &self.entries)
        }
        fn deadline_updated(&mut self, _index: usize) {
            #[cfg(feature = "std")]
            {
                let entries = &self.entries;
                let deadline = entries.get(_index).map(TransportedLink::deadline);
                self.deadline_index.update(_index, deadline, |row| {
                    entries.get(row).map(TransportedLink::deadline)
                });
            }
        }
        fn earliest_indexed_deadline(&mut self) -> Option<crate::engine::InstantMillis> {
            #[cfg(feature = "std")]
            {
                let row_count = self.entries.len();
                let entries = &self.entries;
                self.deadline_index.earliest_exact(row_count, |row| {
                    entries.get(row).map(TransportedLink::deadline)
                })
            }
            #[cfg(not(feature = "std"))]
            self.entries
                .iter()
                .map(TransportedLink::deadline)
                .min_by_key(|deadline| deadline.0)
        }
        fn first_overdue(&mut self, now: crate::engine::InstantMillis) -> Option<usize> {
            #[cfg(feature = "std")]
            {
                let row_count = self.entries.len();
                let entries = &self.entries;
                self.deadline_index.first_due(row_count, now, |row| {
                    entries.get(row).map(TransportedLink::deadline)
                })
            }
            #[cfg(not(feature = "std"))]
            self.entries
                .iter()
                .position(|entry| entry.deadline().0 <= now.0)
        }
        fn push(&mut self, entry: TransportedLink) -> Result<(), TablePushError> {
            let slot = self.entries.len();
            #[cfg(feature = "std")]
            let deadline = entry.deadline();
            self.entries.push(entry);
            self.index.insert(slot, &self.entries);
            #[cfg(feature = "std")]
            {
                let entries = &self.entries;
                self.deadline_index.insert(slot, Some(deadline), |row| {
                    entries.get(row).map(TransportedLink::deadline)
                });
            }
            Ok(())
        }
        fn swap_remove(&mut self, index: usize) {
            if index >= self.entries.len() {
                return;
            }
            let last = self.entries.len() - 1;
            self.index.remove_slot(index, &self.entries);
            if index != last {
                self.index.repoint_slot(last, index, &self.entries);
            }
            #[cfg(feature = "std")]
            {
                let entries = &self.entries;
                self.deadline_index.swap_remove(index, last, |row| {
                    entries.get(row).map(TransportedLink::deadline)
                });
            }
            self.entries.swap_remove(index);
        }
    }
}

#[cfg(feature = "alloc")]
pub use heap_transit_link_columns::*;
