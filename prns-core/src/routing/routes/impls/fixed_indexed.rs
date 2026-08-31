use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::LemireIndex;
use crate::routing::routes::{route_index_buckets, RouteEntry, RouteEvidenceId, RouteTable};
use crate::routing::{NextHop, RouteResponsiveness, RouteRetention};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedIndexedRouteTable<const N: usize, const BUCKETS: usize> {
    len: usize,
    destination: [DestinationHash; N],
    hops: [u8; N],
    learned_at: [InstantMillis; N],
    last_route_activity_at: [InstantMillis; N],
    responsiveness: [RouteResponsiveness; N],
    receiving_interface: [InterfaceId; N],
    next_hop: [NextHop; N],
    retention: [RouteRetention; N],
    evidence_id: [RouteEvidenceId; N],
    index: LemireIndex<BUCKETS>,
}

impl<const N: usize, const BUCKETS: usize> Default for FixedIndexedRouteTable<N, BUCKETS> {
    fn default() -> Self {
        const {
            assert!(
                BUCKETS >= route_index_buckets(N),
                "BUCKETS must give the index its 2/3-load headroom over N: size it with route_index_buckets(N)",
            );
            assert!(
                N < u16::MAX as usize,
                "FixedIndexedRouteTable indexes slots as u16; keep N below 65535",
            );
        }
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); N],
            hops: [0u8; N],
            learned_at: [InstantMillis(0); N],
            last_route_activity_at: [InstantMillis(0); N],
            responsiveness: [RouteResponsiveness::Responsive; N],
            receiving_interface: [InterfaceId::new([0u8; 8]); N],
            next_hop: [NextHop::Direct; N],
            retention: [RouteRetention::Network; N],
            evidence_id: [RouteEvidenceId::FIRST; N],
            index: LemireIndex::default(),
        }
    }
}

impl<const N: usize, const BUCKETS: usize> RouteTable for FixedIndexedRouteTable<N, BUCKETS> {
    fn capacity(&self) -> usize {
        N
    }
    fn len(&self) -> usize {
        self.len
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destination[..])
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination[..self.len]
    }
    fn hops(&self) -> &[u8] {
        &self.hops[..self.len]
    }
    fn learned_at(&self) -> &[InstantMillis] {
        &self.learned_at[..self.len]
    }
    fn last_route_activity_at(&self) -> &[InstantMillis] {
        &self.last_route_activity_at[..self.len]
    }
    fn responsiveness(&self) -> &[RouteResponsiveness] {
        &self.responsiveness[..self.len]
    }
    fn receiving_interfaces(&self) -> &[InterfaceId] {
        &self.receiving_interface[..self.len]
    }
    fn next_hops(&self) -> &[NextHop] {
        &self.next_hop[..self.len]
    }
    fn retentions(&self) -> &[RouteRetention] {
        &self.retention[..self.len]
    }
    fn evidence_ids(&self) -> &[RouteEvidenceId] {
        &self.evidence_id[..self.len]
    }

    fn set_row(&mut self, i: usize, row: RouteEntry) {
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
        if self.len >= N {
            return Err(TablePushError::TableFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.evidence_id[i] = evidence_id;
        self.set_row(i, row);
        self.len += 1;
        self.index.insert(i, &self.destination[..]);
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.len - 1);
        let removed = self.destination[i];
        self.index.remove(&removed, &self.destination[..]);
        if i != last {
            let moved = self.destination[last];
            self.index.repoint(&moved, i, &self.destination[..]);
        }
        self.destination[i] = self.destination[last];
        self.hops[i] = self.hops[last];
        self.learned_at[i] = self.learned_at[last];
        self.last_route_activity_at[i] = self.last_route_activity_at[last];
        self.responsiveness[i] = self.responsiveness[last];
        self.receiving_interface[i] = self.receiving_interface[last];
        self.next_hop[i] = self.next_hop[last];
        self.retention[i] = self.retention[last];
        self.evidence_id[i] = self.evidence_id[last];
        self.len = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU32;

    use crate::routing::routes::route_index_buckets;
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

    type Routes8 = FixedIndexedRouteTable<8, { route_index_buckets(8) }>;

    #[test]
    fn push_exposes_only_pushed_rows_and_finds_them_by_index() {
        let mut table = Routes8::default();
        assert_eq!(table.capacity(), 8);
        assert!(table.is_empty());

        assert_eq!(
            table.push(dest(0xA1), evidence(1), row(1, 10, iface(0xE1))),
            Ok(0)
        );
        assert_eq!(
            table.push(dest(0xB2), evidence(2), row(2, 20, iface(0xE2))),
            Ok(1)
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(0xA1), dest(0xB2)]);
        assert_eq!(table.hops(), &[1, 2]);
        assert_eq!(table.retentions(), &[retention(10), retention(20)]);
        assert_eq!(table.index_of(&dest(0xA1)), Some(0));
        assert_eq!(table.index_of(&dest(0xB2)), Some(1));
        assert_eq!(table.index_of(&dest(0xFF)), None);
    }

    #[test]
    fn the_index_resolves_every_key_through_probe_collisions() {
        let mut table = Routes8::default();
        for n in 0..8u32 {
            assert_eq!(
                table.push(dest_n(n), evidence(n), row(1, n as u64, iface(n as u8))),
                Ok(n as usize)
            );
        }
        for n in 0..8u32 {
            assert_eq!(table.index_of(&dest_n(n)), Some(n as usize));
        }
        assert_eq!(table.index_of(&dest_n(999)), None);
    }

    #[test]
    fn a_full_table_still_terminates_an_absent_lookup() {
        let mut table = Routes8::default();
        for n in 0..8u32 {
            table
                .push(dest_n(n), evidence(n), row(1, n as u64, iface(n as u8)))
                .unwrap();
        }
        assert_eq!(table.len(), 8);
        assert_eq!(
            table.push(dest_n(8), evidence(8), row(1, 8, iface(8))),
            Err(TablePushError::TableFull)
        );
        assert_eq!(table.index_of(&dest_n(8)), None);
        assert_eq!(table.index_of(&dest_n(12345)), None);
    }

    #[test]
    fn swap_remove_moves_the_last_row_and_keeps_the_index_consistent() {
        let mut table = Routes8::default();
        table
            .push(dest_n(1), evidence(1), row(1, 10, iface(0xE1)))
            .unwrap();
        table
            .push(dest_n(2), evidence(2), row(2, 20, iface(0xE2)))
            .unwrap();
        table
            .push(dest_n(3), evidence(3), row(3, 30, iface(0xE3)))
            .unwrap();

        table.swap_remove(0, table.len() - 1);

        assert_eq!(table.len(), 2);
        assert_eq!(table.index_of(&dest_n(1)), None);
        assert_eq!(table.index_of(&dest_n(3)), Some(0));
        assert_eq!(table.index_of(&dest_n(2)), Some(1));
        assert_eq!(table.hops()[table.index_of(&dest_n(3)).unwrap()], 3);
        assert_eq!(table.retentions(), &[retention(30), retention(20)]);
        assert_eq!(table.evidence_ids(), &[evidence(3), evidence(2)]);
    }

    #[test]
    fn churn_keeps_every_surviving_key_findable() {
        let mut table = Routes8::default();
        for n in 0..8u32 {
            table
                .push(dest_n(n), evidence(n), row(1, n as u64, iface(n as u8)))
                .unwrap();
        }
        for _ in 0..4 {
            let victim = table.index_of(&dest_n(0)).unwrap();
            table.swap_remove(victim, table.len() - 1);
            for n in 1..8u32 {
                assert_eq!(table.hops()[table.index_of(&dest_n(n)).unwrap()], 1);
            }
            table
                .push(dest_n(0), evidence(9), row(7, 70, iface(0)))
                .unwrap();
            assert_eq!(table.hops()[table.index_of(&dest_n(0)).unwrap()], 7);
        }
    }
}
