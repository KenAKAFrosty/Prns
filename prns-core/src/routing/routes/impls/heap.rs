//! Heap-backed, growable routing table: the same SoA shape as
//! [`FixedArrayRouteTable`](super::FixedArrayRouteTable) with no ceiling, so the
//! engine's drop-when-full check never trips and `push` cannot fail.
//!
//! At relay scale the destination lookup is the hot op, so this backend carries a
//! [`HeapLemireIndex`] over the destination column.
//! The fixed backend keeps the default linear scan, which wins at small N.

use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::routes::{RouteEntry, RouteTable};
use crate::routing::{NextHop, RouteResponsiveness};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRouteTable {
    destination: Vec<DestinationHash>,
    hops: Vec<u8>,
    learned_at: Vec<InstantMillis>,
    last_relayed_at: Vec<InstantMillis>,
    responsiveness: Vec<RouteResponsiveness>,
    receiving_interface: Vec<InterfaceId>,
    next_hop: Vec<NextHop>,
    index: HeapLemireIndex,
}

impl RouteTable for HeapRouteTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destination.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destination)
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
    fn last_relayed_at(&self) -> &[InstantMillis] {
        &self.last_relayed_at
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

    fn set_row(&mut self, i: usize, row: RouteEntry) {
        self.hops[i] = row.hops;
        self.learned_at[i] = row.learned_at;
        self.last_relayed_at[i] = row.last_relayed_at;
        self.responsiveness[i] = row.responsiveness;
        self.receiving_interface[i] = row.receiving_interface;
        self.next_hop[i] = row.next_hop;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        row: RouteEntry,
    ) -> Result<usize, TablePushError> {
        let i = self.destination.len();
        self.destination.push(destination);
        self.hops.push(row.hops);
        self.learned_at.push(row.learned_at);
        self.last_relayed_at.push(row.last_relayed_at);
        self.responsiveness.push(row.responsiveness);
        self.receiving_interface.push(row.receiving_interface);
        self.next_hop.push(row.next_hop);
        self.index.insert(i, &self.destination);
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.destination.len() - 1);
        let removed = self.destination[i];
        self.index.remove(&removed, &self.destination);
        if i != last {
            let moved = self.destination[last];
            self.index.repoint(&moved, i, &self.destination);
        }
        self.destination.swap_remove(i);
        self.hops.swap_remove(i);
        self.learned_at.swap_remove(i);
        self.last_relayed_at.swap_remove(i);
        self.responsiveness.swap_remove(i);
        self.receiving_interface.swap_remove(i);
        self.next_hop.swap_remove(i);
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
    fn row(hops: u8, learned_at: u64, receiving_interface: InterfaceId) -> RouteEntry {
        RouteEntry {
            hops,
            learned_at: InstantMillis(learned_at),
            last_relayed_at: InstantMillis(0),
            responsiveness: RouteResponsiveness::Responsive,
            receiving_interface,
            next_hop: NextHop::Direct,
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
        assert_eq!(table.capacity(), usize::MAX);
        assert!(table.is_empty());

        for n in 0..1_000u32 {
            assert_eq!(
                table.push(dest_n(n), row(1, n as u64, iface(n as u8))),
                Ok(n as usize)
            );
        }
        assert_eq!(table.len(), 1_000);
        assert_eq!(table.destinations().len(), 1_000);

        table.set_row(0, row(9, 99, iface(0xEE)));
        assert_eq!(table.hops()[0], 9);
        assert_eq!(table.receiving_interfaces()[0], iface(0xEE));
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut table = HeapRouteTable::default();
        table.push(dest(0xA1), row(1, 10, iface(0xE1))).unwrap();
        table.push(dest(0xB2), row(2, 20, iface(0xE2))).unwrap();
        table.push(dest(0xC3), row(3, 30, iface(0xE3))).unwrap();

        table.swap_remove(0, table.len() - 1);

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(0xC3), dest(0xB2)]);
        assert_eq!(table.hops(), &[3, 2]);
        assert_eq!(table.learned_at(), &[InstantMillis(30), InstantMillis(20)]);
        assert_eq!(table.receiving_interfaces(), &[iface(0xE3), iface(0xE2)]);
    }

    #[test]
    fn the_index_finds_inserted_destinations_and_misses_absent_ones() {
        let mut table = HeapRouteTable::default();
        let a = table.push(dest_n(1), row(1, 10, iface(0))).unwrap();
        let b = table.push(dest_n(2), row(2, 20, iface(0))).unwrap();

        assert_eq!(table.index_of(&dest_n(1)), Some(a));
        assert_eq!(table.index_of(&dest_n(2)), Some(b));
        assert_eq!(table.index_of(&dest_n(999)), None);
    }

    #[test]
    fn the_index_tracks_a_swap_remove() {
        let mut table = HeapRouteTable::default();
        table.push(dest_n(1), row(1, 10, iface(0))).unwrap();
        table.push(dest_n(2), row(2, 20, iface(0))).unwrap();
        table.push(dest_n(3), row(3, 30, iface(0))).unwrap();

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
                let slot = table.push(dest_n(id), row(1, id as u64, iface(0))).unwrap();
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
}
