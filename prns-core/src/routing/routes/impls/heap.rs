use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::routes::interface_index::{LinearRouteInterfaceIndex, RouteInterfaceIndex};
use crate::routing::routes::{RouteEntry, RouteEvidenceId, RouteTable};
use crate::routing::{NextHop, RouteResponsiveness, RouteRetention};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRouteTableWithInterfaceIndex<I> {
    destination: Vec<DestinationHash>,
    hops: Vec<u8>,
    learned_at: Vec<InstantMillis>,
    last_route_activity_at: Vec<InstantMillis>,
    responsiveness: Vec<RouteResponsiveness>,
    receiving_interface: Vec<InterfaceId>,
    next_hop: Vec<NextHop>,
    retention: Vec<RouteRetention>,
    evidence_id: Vec<RouteEvidenceId>,
    index: HeapLemireIndex,
    interface_index: I,
}

pub type LinearHeapRouteTable = HeapRouteTableWithInterfaceIndex<LinearRouteInterfaceIndex>;

#[cfg(feature = "std")]
pub type RoaringHeapRouteTable = HeapRouteTableWithInterfaceIndex<
    crate::routing::routes::interface_index::RoaringRouteInterfaceIndex,
>;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        pub type HeapRouteTable = RoaringHeapRouteTable;
    } else {
        pub type HeapRouteTable = LinearHeapRouteTable;
    }
}

impl<I: RouteInterfaceIndex> RouteTable for HeapRouteTableWithInterfaceIndex<I> {
    fn capacity(&self) -> usize {
        HeapLemireIndex::MAX_ROWS
    }
    fn len(&self) -> usize {
        self.destination.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destination)
    }

    fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.interface_index
            .route_count_via(interface, &self.receiving_interface)
    }

    fn repoint_receiving_interface(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
    ) -> usize {
        self.interface_index.repoint_receiving_interface(
            previous,
            current,
            now,
            &mut self.receiving_interface,
            &mut self.last_route_activity_at,
        )
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination
    }
    fn hops(&self) -> &[u8] {
        &self.hops
    }
    fn learned_at(&self) -> &[InstantMillis] {
        &self.learned_at
    }
    fn last_route_activity_at(&self) -> &[InstantMillis] {
        &self.last_route_activity_at
    }
    fn responsiveness(&self) -> &[RouteResponsiveness] {
        &self.responsiveness
    }
    fn receiving_interfaces(&self) -> &[InterfaceId] {
        &self.receiving_interface
    }
    fn next_hops(&self) -> &[NextHop] {
        &self.next_hop
    }
    fn retentions(&self) -> &[RouteRetention] {
        &self.retention
    }
    fn evidence_ids(&self) -> &[RouteEvidenceId] {
        &self.evidence_id
    }

    fn set_row(&mut self, i: usize, row: RouteEntry) {
        self.interface_index
            .update(i, self.receiving_interface[i], row.receiving_interface);
        self.hops[i] = row.hops;
        self.learned_at[i] = row.learned_at;
        self.last_route_activity_at[i] = row.last_route_activity_at;
        self.responsiveness[i] = row.responsiveness;
        self.receiving_interface[i] = row.receiving_interface;
        self.next_hop[i] = row.next_hop;
        self.retention[i] = row.retention;
    }

    fn set_evidence_id(&mut self, i: usize, evidence_id: RouteEvidenceId) {
        self.evidence_id[i] = evidence_id;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        evidence_id: RouteEvidenceId,
        row: RouteEntry,
    ) -> Result<usize, TablePushError> {
        if self.destination.len() >= self.capacity() {
            return Err(TablePushError::TableFull);
        }
        let i = self.destination.len();
        self.destination.push(destination);
        self.evidence_id.push(evidence_id);
        self.hops.push(row.hops);
        self.learned_at.push(row.learned_at);
        self.last_route_activity_at.push(row.last_route_activity_at);
        self.responsiveness.push(row.responsiveness);
        self.receiving_interface.push(row.receiving_interface);
        self.next_hop.push(row.next_hop);
        self.retention.push(row.retention);
        self.index.insert(i, &self.destination);
        self.interface_index.insert(i, row.receiving_interface);
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.destination.len() - 1);
        self.interface_index
            .swap_remove(i, last, &self.receiving_interface);
        let removed = self.destination[i];
        self.index.remove(&removed, &self.destination);
        if i != last {
            let moved = self.destination[last];
            self.index.repoint(&moved, i, &self.destination);
        }
        self.destination.swap_remove(i);
        self.hops.swap_remove(i);
        self.learned_at.swap_remove(i);
        self.last_route_activity_at.swap_remove(i);
        self.responsiveness.swap_remove(i);
        self.receiving_interface.swap_remove(i);
        self.next_hop.swap_remove(i);
        self.retention.swap_remove(i);
        self.evidence_id.swap_remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU32;

    use crate::routing::RouteExpiresAfter;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }
    fn evidence(n: u32) -> RouteEvidenceId {
        RouteEvidenceId::new(n + 1).unwrap()
    }
    fn retention(learned_at: u64) -> RouteRetention {
        RouteRetention::Ephemeral {
            expires_after: RouteExpiresAfter::from_nonzero_millis(
                NonZeroU32::new((learned_at as u32).saturating_add(1)).unwrap(),
            ),
        }
    }
    fn row(hops: u8, learned_at: u64, receiving_interface: InterfaceId) -> RouteEntry {
        RouteEntry {
            hops,
            learned_at: InstantMillis(learned_at),
            last_route_activity_at: InstantMillis(0),
            responsiveness: RouteResponsiveness::Responsive,
            receiving_interface,
            next_hop: NextHop::Direct,
            retention: retention(learned_at),
        }
    }

    fn dest_n(n: u32) -> DestinationHash {
        let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&key.to_be_bytes());
        b[8..12].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(b)
    }

    #[test]
    fn grows_past_any_fixed_ceiling_and_exposes_only_pushed_rows() {
        let mut table = HeapRouteTable::default();
        assert_eq!(table.capacity(), HeapLemireIndex::MAX_ROWS);
        assert!(table.is_empty());

        for n in 0..1_000u32 {
            assert_eq!(
                table.push(dest_n(n), evidence(n), row(1, n as u64, iface(n as u8))),
                Ok(n as usize)
            );
        }
        assert_eq!(table.len(), 1_000);
        assert_eq!(table.destinations().len(), 1_000);

        table.set_row(0, row(9, 99, iface(0xEE)));
        assert_eq!(table.hops()[0], 9);
        assert_eq!(table.receiving_interfaces()[0], iface(0xEE));
        assert_eq!(table.retentions()[0], retention(99));
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut table = HeapRouteTable::default();
        table
            .push(dest(0xA1), evidence(1), row(1, 10, iface(0xE1)))
            .unwrap();
        table
            .push(dest(0xB2), evidence(2), row(2, 20, iface(0xE2)))
            .unwrap();
        table
            .push(dest(0xC3), evidence(3), row(3, 30, iface(0xE3)))
            .unwrap();

        table.swap_remove(0, table.len() - 1);

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(0xC3), dest(0xB2)]);
        assert_eq!(table.hops(), &[3, 2]);
        assert_eq!(table.learned_at(), &[InstantMillis(30), InstantMillis(20)]);
        assert_eq!(table.receiving_interfaces(), &[iface(0xE3), iface(0xE2)]);
        assert_eq!(table.retentions(), &[retention(30), retention(20)]);
        assert_eq!(table.evidence_ids(), &[evidence(3), evidence(2)]);
    }

    #[test]
    fn the_index_finds_inserted_destinations_and_misses_absent_ones() {
        let mut table = HeapRouteTable::default();
        let a = table
            .push(dest_n(1), evidence(1), row(1, 10, iface(0)))
            .unwrap();
        let b = table
            .push(dest_n(2), evidence(2), row(2, 20, iface(0)))
            .unwrap();

        assert_eq!(table.index_of(&dest_n(1)), Some(a));
        assert_eq!(table.index_of(&dest_n(2)), Some(b));
        assert_eq!(table.index_of(&dest_n(999)), None);
    }

    #[test]
    fn the_index_tracks_a_swap_remove() {
        let mut table = HeapRouteTable::default();
        table
            .push(dest_n(1), evidence(1), row(1, 10, iface(0)))
            .unwrap();
        table
            .push(dest_n(2), evidence(2), row(2, 20, iface(0)))
            .unwrap();
        table
            .push(dest_n(3), evidence(3), row(3, 30, iface(0)))
            .unwrap();

        table.swap_remove(0, table.len() - 1);

        assert_eq!(table.index_of(&dest_n(1)), None, "the removed dest is gone");
        assert_eq!(
            table.index_of(&dest_n(3)),
            Some(0),
            "the dest swapped into the hole is found at its new slot",
        );
        assert_eq!(table.index_of(&dest_n(2)), Some(1));
    }

    #[test]
    fn the_index_stays_consistent_through_many_inserts_and_removes() {
        let mut table = HeapRouteTable::default();
        let mut live: std::vec::Vec<u32> = std::vec::Vec::new();
        let mut rng = 0x0123_4567_89AB_CDEFu64;
        let mut next_id = 0u32;

        for _ in 0..1_000 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let insert = live.len() < 2 || !(rng >> 33).is_multiple_of(3);
            if insert {
                let id = next_id;
                next_id += 1;
                let slot = table
                    .push(dest_n(id), evidence(id), row(1, id as u64, iface(0)))
                    .unwrap();
                assert_eq!(slot, live.len());
                live.push(id);
            } else {
                let victim = ((rng >> 17) as usize) % live.len();
                table.swap_remove(victim, table.len() - 1);
                live.swap_remove(victim);
            }

            for (slot, &id) in live.iter().enumerate() {
                assert_eq!(
                    table.index_of(&dest_n(id)),
                    Some(slot),
                    "every live destination resolves to its current slot",
                );
            }
            assert_eq!(table.index_of(&dest_n(next_id + 7)), None);
        }
        assert!(
            live.len() > 50,
            "the run must grow enough to force reindexing"
        );
    }

    #[cfg(feature = "std")]
    fn assert_route_tables_match(linear: &LinearHeapRouteTable, roaring: &RoaringHeapRouteTable) {
        assert_eq!(linear.destinations(), roaring.destinations());
        assert_eq!(linear.hops(), roaring.hops());
        assert_eq!(linear.learned_at(), roaring.learned_at());
        assert_eq!(
            linear.last_route_activity_at(),
            roaring.last_route_activity_at()
        );
        assert_eq!(linear.responsiveness(), roaring.responsiveness());
        assert_eq!(
            linear.receiving_interfaces(),
            roaring.receiving_interfaces()
        );
        assert_eq!(linear.next_hops(), roaring.next_hops());
        for interface in 0..=u8::MAX {
            let interface = iface(interface);
            assert_eq!(
                linear.route_count_via(interface),
                roaring.route_count_via(interface)
            );
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn roaring_interface_membership_matches_linear_scans_through_route_churn() {
        let mut linear = LinearHeapRouteTable::default();
        let mut roaring = RoaringHeapRouteTable::default();
        let mut rng = 0xA076_1D64_78BD_642Fu64;
        let mut next_id = 0u32;

        for step in 0..4_000u64 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match if linear.len() < 4 {
                0
            } else {
                (rng >> 61) as u8
            } {
                0..=2 => {
                    let interface = iface((rng >> 17) as u8);
                    let route = row(1, step, interface);
                    let slot = linear.len();
                    assert_eq!(
                        linear.push(dest_n(next_id), evidence(next_id), route),
                        Ok(slot)
                    );
                    assert_eq!(
                        roaring.push(dest_n(next_id), evidence(next_id), route),
                        Ok(slot)
                    );
                    next_id += 1;
                }
                3..=4 => {
                    let slot = ((rng >> 23) as usize) % linear.len();
                    let route = RouteEntry {
                        hops: linear.hops()[slot],
                        learned_at: linear.learned_at()[slot],
                        last_route_activity_at: InstantMillis(step),
                        responsiveness: linear.responsiveness()[slot],
                        receiving_interface: iface((rng >> 37) as u8),
                        next_hop: linear.next_hops()[slot],
                        retention: linear.retentions()[slot],
                    };
                    linear.set_row(slot, route);
                    roaring.set_row(slot, route);
                }
                5..=6 => {
                    let slot = ((rng >> 29) as usize) % linear.len();
                    let last = linear.len() - 1;
                    linear.swap_remove(slot, last);
                    roaring.swap_remove(slot, last);
                }
                _ => {
                    let previous = iface((rng >> 11) as u8);
                    let current = if rng & 1 == 0 {
                        previous
                    } else {
                        iface(((rng >> 11) as u8).wrapping_add(1))
                    };
                    assert_eq!(
                        linear.repoint_receiving_interface(previous, current, InstantMillis(step)),
                        roaring.repoint_receiving_interface(previous, current, InstantMillis(step))
                    );
                }
            }
            assert_route_tables_match(&linear, &roaring);
        }
    }
}
