//! Heap-backed, growable routing-table columns (the typical std/alloc backend).
//!
//! The same SoA shape as [`FixedArrayRouteColumns`](super::FixedArrayRouteColumns),
//! one `Vec` per column joined by slot index, but with no ceiling: the table grows
//! with the network. `capacity()` is `usize::MAX`, so the engine's drop-when-full
//! check (`len >= capacity`) never trips, and `push` cannot fail.
//!
//! At relay scale the destination lookup is the hot op, so this backend carries a
//! side index — an open-addressing table of slots keyed by the destination's own
//! leading bytes (destinations are truncated SHA-256, already uniform, so no hash
//! function is needed; the bucket is a Lemire multiply-shift reduction). It keeps
//! load below ~2/3 by doubling and reindexing, probes linearly, and deletes by
//! backward-shift so a long-lived churning table never silts up with tombstones.
//! The fixed backend keeps `index_of`'s default linear scan, which wins at small N.

use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::storage::ColumnsFull;
use crate::routing::routes::{RouteColumns, RouteEntry};
use crate::routing::{NextHop, RouteResponsiveness};
use crate::wire::DestinationHash;

const EMPTY: usize = usize::MAX;
const MIN_BUCKETS: usize = 8;

#[derive(Debug)]
pub struct HeapRouteColumns {
    destination: Vec<DestinationHash>,
    hops: Vec<u8>,
    learned_at: Vec<InstantMillis>,
    responsiveness: Vec<RouteResponsiveness>,
    receiving_interface: Vec<InterfaceId>,
    next_hop: Vec<NextHop>,
    index: Vec<usize>,
}

impl Default for HeapRouteColumns {
    fn default() -> Self {
        let mut index = Vec::new();
        index.resize(MIN_BUCKETS, EMPTY);
        Self {
            destination: Vec::new(),
            hops: Vec::new(),
            learned_at: Vec::new(),
            responsiveness: Vec::new(),
            receiving_interface: Vec::new(),
            next_hop: Vec::new(),
            index,
        }
    }
}

impl HeapRouteColumns {
    fn key(destination: &DestinationHash) -> u64 {
        let b = destination.as_bytes();
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    }

    fn bucket(&self, key: u64) -> usize {
        ((key as u128 * self.index.len() as u128) >> 64) as usize
    }

    fn index_position(&self, destination: &DestinationHash) -> Option<usize> {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(destination));
        loop {
            let slot = self.index[pos];
            if slot == EMPTY {
                return None;
            }
            if self.destination[slot] == *destination {
                return Some(pos);
            }
            pos = (pos + 1) % n;
        }
    }

    fn index_insert(&mut self, slot: usize) {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(&self.destination[slot]));
        while self.index[pos] != EMPTY {
            pos = (pos + 1) % n;
        }
        self.index[pos] = slot;
    }

    fn index_delete(&mut self, destination: &DestinationHash) {
        let Some(mut hole) = self.index_position(destination) else {
            return;
        };
        let n = self.index.len();
        loop {
            self.index[hole] = EMPTY;
            let mut scan = hole;
            loop {
                scan = (scan + 1) % n;
                let slot = self.index[scan];
                if slot == EMPTY {
                    return;
                }
                let home = self.bucket(Self::key(&self.destination[slot]));
                let blocks_move = if hole <= scan {
                    home > hole && home <= scan
                } else {
                    home > hole || home <= scan
                };
                if !blocks_move {
                    self.index[hole] = slot;
                    hole = scan;
                    break;
                }
            }
        }
    }

    fn index_repoint(&mut self, destination: &DestinationHash, slot: usize) {
        if let Some(pos) = self.index_position(destination) {
            self.index[pos] = slot;
        }
    }

    fn grow_index_if_loaded(&mut self) {
        if (self.destination.len() + 1) * 3 > self.index.len() * 2 {
            let new_buckets = self.index.len() * 2;
            self.index.clear();
            self.index.resize(new_buckets, EMPTY);
            for slot in 0..self.destination.len() {
                self.index_insert(slot);
            }
        }
    }
}

impl RouteColumns for HeapRouteColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destination.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index_position(destination).map(|pos| self.index[pos])
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
        self.responsiveness[i] = row.responsiveness;
        self.receiving_interface[i] = row.receiving_interface;
        self.next_hop[i] = row.next_hop;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        row: RouteEntry,
    ) -> Result<usize, ColumnsFull> {
        self.grow_index_if_loaded();
        let i = self.destination.len();
        self.destination.push(destination);
        self.hops.push(row.hops);
        self.learned_at.push(row.learned_at);
        self.responsiveness.push(row.responsiveness);
        self.receiving_interface.push(row.receiving_interface);
        self.next_hop.push(row.next_hop);
        self.index_insert(i);
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize) {
        let last = self.destination.len() - 1;
        let removed = self.destination[i];
        self.index_delete(&removed);
        if i != last {
            let moved = self.destination[last];
            self.index_repoint(&moved, i);
        }
        self.destination.swap_remove(i);
        self.hops.swap_remove(i);
        self.learned_at.swap_remove(i);
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
        InterfaceId::new([byte; 16])
    }
    fn row(hops: u8, learned_at: u64, receiving_interface: InterfaceId) -> RouteEntry {
        RouteEntry {
            hops,
            learned_at: InstantMillis(learned_at),
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
        let mut columns = HeapRouteColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        assert!(columns.is_empty());

        for n in 0..1_000u32 {
            assert_eq!(
                columns.push(dest_n(n), row(1, n as u64, iface(n as u8))),
                Ok(n as usize)
            );
        }
        assert_eq!(columns.len(), 1_000);
        assert_eq!(columns.destinations().len(), 1_000);

        columns.set_row(0, row(9, 99, iface(0xEE)));
        assert_eq!(columns.hops()[0], 9);
        assert_eq!(columns.receiving_interfaces()[0], iface(0xEE));
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut columns = HeapRouteColumns::default();
        columns.push(dest(0xA1), row(1, 10, iface(0xE1))).unwrap();
        columns.push(dest(0xB2), row(2, 20, iface(0xE2))).unwrap();
        columns.push(dest(0xC3), row(3, 30, iface(0xE3))).unwrap();

        columns.swap_remove(0);

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.destinations(), &[dest(0xC3), dest(0xB2)]);
        assert_eq!(columns.hops(), &[3, 2]);
        assert_eq!(
            columns.learned_at(),
            &[InstantMillis(30), InstantMillis(20)]
        );
        assert_eq!(columns.receiving_interfaces(), &[iface(0xE3), iface(0xE2)]);
    }

    #[test]
    fn the_index_finds_inserted_destinations_and_misses_absent_ones() {
        let mut columns = HeapRouteColumns::default();
        let a = columns.push(dest_n(1), row(1, 10, iface(0))).unwrap();
        let b = columns.push(dest_n(2), row(2, 20, iface(0))).unwrap();

        assert_eq!(columns.index_of(&dest_n(1)), Some(a));
        assert_eq!(columns.index_of(&dest_n(2)), Some(b));
        assert_eq!(columns.index_of(&dest_n(999)), None);
    }

    #[test]
    fn the_index_tracks_a_swap_remove() {
        let mut columns = HeapRouteColumns::default();
        columns.push(dest_n(1), row(1, 10, iface(0))).unwrap();
        columns.push(dest_n(2), row(2, 20, iface(0))).unwrap();
        columns.push(dest_n(3), row(3, 30, iface(0))).unwrap();

        columns.swap_remove(0);

        assert_eq!(
            columns.index_of(&dest_n(1)),
            None,
            "the removed dest is gone"
        );
        assert_eq!(
            columns.index_of(&dest_n(3)),
            Some(0),
            "the dest swapped into the hole is found at its new slot",
        );
        assert_eq!(columns.index_of(&dest_n(2)), Some(1));
    }

    #[test]
    fn the_index_stays_consistent_through_many_inserts_and_removes() {
        let mut columns = HeapRouteColumns::default();
        let mut live: std::vec::Vec<u32> = std::vec::Vec::new();
        let mut rng = 0x1234_5678_9ABC_DEFu64;
        let mut next_id = 0u32;

        for _ in 0..1_000 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let insert = live.len() < 2 || (rng >> 33) % 3 != 0;
            if insert {
                let id = next_id;
                next_id += 1;
                let slot = columns
                    .push(dest_n(id), row(1, id as u64, iface(0)))
                    .unwrap();
                assert_eq!(slot, live.len());
                live.push(id);
            } else {
                let victim = ((rng >> 17) as usize) % live.len();
                columns.swap_remove(victim);
                live.swap_remove(victim);
            }

            for (slot, &id) in live.iter().enumerate() {
                assert_eq!(
                    columns.index_of(&dest_n(id)),
                    Some(slot),
                    "every live destination resolves to its current slot",
                );
            }
            assert_eq!(columns.index_of(&dest_n(next_id + 7)), None);
        }
        assert!(
            live.len() > 50,
            "the run must grow enough to force reindexing"
        );
    }
}
