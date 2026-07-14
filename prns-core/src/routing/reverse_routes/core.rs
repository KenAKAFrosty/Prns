use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

/// RNS 1.3.5 `Transport.REVERSE_TIMEOUT` (8 minutes).
pub const DEFAULT_REVERSE_ROUTE_TIMEOUT_MS: u64 = 8 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReverseRouteEntry {
    pub proof_destination: DestinationHash,
    pub received_interface: InterfaceId,
    pub outbound_interface: InterfaceId,
    pub expires_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReverseRoute {
    pub received_interface: InterfaceId,
    pub outbound_interface: InterfaceId,
}

pub trait ReverseRouteTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn proof_destinations(&self) -> &[DestinationHash];
    fn received_interfaces(&self) -> &[InterfaceId];
    fn outbound_interfaces(&self) -> &[InterfaceId];
    fn expires_ats(&self) -> &[InstantMillis];
    fn push(&mut self, entry: ReverseRouteEntry);
    fn swap_remove(&mut self, index: usize);

    fn index_of(&self, proof_destination: &DestinationHash) -> Option<usize> {
        self.proof_destinations()
            .iter()
            .position(|candidate| candidate == proof_destination)
    }

    fn first_expired(&mut self, now: InstantMillis) -> Option<usize> {
        self.expires_ats()
            .iter()
            .position(|expires_at| *expires_at <= now)
    }
}

#[derive(Debug, Default)]
pub struct ReverseRoutes<C: ReverseRouteTable> {
    table: C,
}

impl<C: ReverseRouteTable> ReverseRoutes<C> {
    /// A full table evicts its soonest-expiring row to make room, always favoring the new packet. A later proof for an evicted row cannot return, so its sender falls back to timeout and resend rather than losing settlement silently.
    pub fn remember(&mut self, entry: ReverseRouteEntry, now: InstantMillis) {
        self.evict_expired(now);
        if self.table.len() >= self.table.capacity() {
            self.evict_soonest_expiring();
        }
        self.table.push(entry);
    }

    pub fn take(
        &mut self,
        proof_destination: &DestinationHash,
        now: InstantMillis,
    ) -> Option<ReverseRoute> {
        let index = self.table.index_of(proof_destination)?;
        let expired = self.table.expires_ats()[index] <= now;
        let route = ReverseRoute {
            received_interface: self.table.received_interfaces()[index],
            outbound_interface: self.table.outbound_interfaces()[index],
        };
        self.table.swap_remove(index);
        if expired {
            return None;
        }
        Some(route)
    }

    pub fn cull_interface_orphans(&mut self, interface_present: impl Fn(InterfaceId) -> bool) {
        let mut index = 0;
        while index < self.table.len() {
            if interface_present(self.table.received_interfaces()[index])
                && interface_present(self.table.outbound_interfaces()[index])
            {
                index += 1;
            } else {
                self.table.swap_remove(index);
            }
        }
    }

    fn evict_expired(&mut self, now: InstantMillis) {
        while let Some(index) = self.table.first_expired(now) {
            self.table.swap_remove(index);
        }
    }

    fn evict_soonest_expiring(&mut self) {
        let Some(index) = self
            .table
            .expires_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, expires_at)| **expires_at)
            .map(|(index, _)| index)
        else {
            return;
        };
        self.table.swap_remove(index);
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
    use super::super::*;
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }
    fn entry(key: u8, received: u8, outbound: u8, expires: u64) -> ReverseRouteEntry {
        ReverseRouteEntry {
            proof_destination: dest(key),
            received_interface: iface(received),
            outbound_interface: iface(outbound),
            expires_at: InstantMillis(expires),
        }
    }

    #[test]
    fn a_remembered_route_pops_exactly_once() {
        let mut table: ReverseRoutes<FixedReverseRouteTable<4>> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB2, 10_000), InstantMillis(1_000));

        assert_eq!(
            table.take(&dest(1), InstantMillis(2_000)),
            Some(ReverseRoute {
                received_interface: iface(0xA1),
                outbound_interface: iface(0xB2),
            }),
        );
        assert_eq!(table.take(&dest(1), InstantMillis(2_000)), None);
        assert!(table.is_empty());
    }

    #[test]
    fn an_expired_route_is_a_miss_and_leaves_no_row() {
        let mut table: ReverseRoutes<FixedReverseRouteTable<4>> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB2, 10_000), InstantMillis(1_000));

        assert_eq!(table.take(&dest(1), InstantMillis(10_000)), None);
        assert!(table.is_empty());
    }

    #[test]
    fn a_full_table_evicts_the_soonest_expiring_row_for_the_new_packet() {
        let mut table: ReverseRoutes<FixedReverseRouteTable<2>> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB1, 30_000), InstantMillis(1_000));
        table.remember(entry(2, 0xA2, 0xB2, 20_000), InstantMillis(1_000));
        table.remember(entry(3, 0xA3, 0xB3, 40_000), InstantMillis(1_000));

        assert_eq!(table.len(), 2);
        assert_eq!(table.take(&dest(2), InstantMillis(2_000)), None);
        assert!(table.take(&dest(1), InstantMillis(2_000)).is_some());
        assert!(table.take(&dest(3), InstantMillis(2_000)).is_some());
    }

    #[test]
    fn remembering_sweeps_expired_rows_before_capacity_counts() {
        let mut table: ReverseRoutes<FixedReverseRouteTable<2>> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB1, 5_000), InstantMillis(1_000));
        table.remember(entry(2, 0xA2, 0xB2, 30_000), InstantMillis(1_000));

        table.remember(entry(3, 0xA3, 0xB3, 40_000), InstantMillis(6_000));
        assert_eq!(table.len(), 2);
        assert!(table.take(&dest(2), InstantMillis(7_000)).is_some());
        assert!(table.take(&dest(3), InstantMillis(7_000)).is_some());
    }

    #[test]
    fn heap_columns_grow_past_any_fixed_ceiling() {
        let mut table: ReverseRoutes<HeapReverseRouteTable> = ReverseRoutes::default();
        for n in 0..64u8 {
            table.remember(entry(n, n, n, 100_000), InstantMillis(1_000));
        }
        assert_eq!(table.len(), 64);
        assert!(table.take(&dest(17), InstantMillis(2_000)).is_some());
        assert_eq!(table.len(), 63);
    }

    #[test]
    fn heap_lookup_preserves_duplicate_proof_destinations_across_swap_removes() {
        let mut table: ReverseRoutes<HeapReverseRouteTable> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB1, 30_000), InstantMillis(1_000));
        table.remember(entry(2, 0xA2, 0xB2, 40_000), InstantMillis(1_000));
        table.remember(entry(1, 0xA3, 0xB3, 50_000), InstantMillis(1_000));

        assert!(table.take(&dest(1), InstantMillis(2_000)).is_some());
        assert!(table.take(&dest(1), InstantMillis(2_000)).is_some());
        assert!(table.take(&dest(2), InstantMillis(2_000)).is_some());
        assert!(table.is_empty());
    }

    #[cfg(feature = "std")]
    #[test]
    fn heap_expiry_index_culls_exact_boundaries_after_row_moves() {
        let mut table: ReverseRoutes<HeapReverseRouteTable> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB1, 10_000), InstantMillis(1_000));
        table.remember(entry(2, 0xA2, 0xB2, 20_000), InstantMillis(1_000));
        table.remember(entry(3, 0xA3, 0xB3, 15_000), InstantMillis(1_000));

        table.remember(entry(4, 0xA4, 0xB4, 30_000), InstantMillis(10_000));
        assert_eq!(table.take(&dest(1), InstantMillis(10_000)), None);
        assert!(table.take(&dest(3), InstantMillis(14_999)).is_some());
        assert!(table.take(&dest(2), InstantMillis(19_999)).is_some());
        assert!(table.take(&dest(4), InstantMillis(29_999)).is_some());
    }

    #[test]
    fn a_reverse_route_whose_interface_left_the_view_is_culled() {
        let mut table: ReverseRoutes<FixedReverseRouteTable<4>> = ReverseRoutes::default();
        table.remember(entry(1, 0xA1, 0xB2, 30_000), InstantMillis(1_000));
        table.remember(entry(2, 0xA1, 0xEE, 30_000), InstantMillis(1_000));

        table.cull_interface_orphans(|id| id != iface(0xEE));

        assert_eq!(
            table.len(),
            1,
            "the row whose outbound lane vanished is gone"
        );
        assert!(table.take(&dest(1), InstantMillis(2_000)).is_some());
        assert_eq!(table.take(&dest(2), InstantMillis(2_000)), None);
    }
}
