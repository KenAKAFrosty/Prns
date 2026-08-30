use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::routes::{RouteEntry, RouteEvidenceId, RouteTable};
use crate::routing::{NextHop, RouteResponsiveness, RouteRetention};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

/// `PartialEq` is structural: every slot compares, including unused tail past `len`. Determinism tests rely on this exactly as `RoutingTable` already does; it is not "same set of destinations."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayRouteTable<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    learned_at: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    last_route_activity_at: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [RouteResponsiveness; MAX_TRACKED_DESTINATIONS],
    receiving_interface: [InterfaceId; MAX_TRACKED_DESTINATIONS],
    next_hop: [NextHop; MAX_TRACKED_DESTINATIONS],
    retention: [RouteRetention; MAX_TRACKED_DESTINATIONS],
    evidence_id: [RouteEvidenceId; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayRouteTable<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            learned_at: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            last_route_activity_at: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [RouteResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            receiving_interface: [InterfaceId::new([0u8; 8]); MAX_TRACKED_DESTINATIONS],
            next_hop: [NextHop::Direct; MAX_TRACKED_DESTINATIONS],
            retention: [RouteRetention::Network; MAX_TRACKED_DESTINATIONS],
            evidence_id: [RouteEvidenceId::FIRST; MAX_TRACKED_DESTINATIONS],
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize> RouteTable
    for FixedArrayRouteTable<MAX_TRACKED_DESTINATIONS>
{
    fn capacity(&self) -> usize {
        MAX_TRACKED_DESTINATIONS
    }
    fn len(&self) -> usize {
        self.len
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
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(TablePushError::TableFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.evidence_id[i] = evidence_id;
        self.set_row(i, row);
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.len - 1);
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

    fn row(
        hops: u8,
        learned_at: u64,
        responsiveness: RouteResponsiveness,
        receiving_interface: InterfaceId,
    ) -> RouteEntry {
        RouteEntry {
            hops,
            learned_at: InstantMillis(learned_at),
            last_route_activity_at: InstantMillis(0),
            responsiveness,
            receiving_interface,
            next_hop: NextHop::Direct,
            retention: retention(learned_at),
        }
    }

    #[test]
    fn push_exposes_only_initialized_rows() {
        let mut table: FixedArrayRouteTable<3> = FixedArrayRouteTable::default();

        assert_eq!(table.capacity(), 3);
        assert!(table.is_empty());
        assert_eq!(
            table.push(
                dest(0xA1),
                RouteEvidenceId::FIRST,
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1))
            ),
            Ok(0)
        );
        assert_eq!(
            table.push(
                dest(0xB2),
                RouteEvidenceId::FIRST,
                row(2, 20, RouteResponsiveness::Unresponsive, iface(0xE2))
            ),
            Ok(1)
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(0xA1), dest(0xB2)]);
        assert_eq!(table.hops(), &[1, 2]);
        assert_eq!(table.learned_at(), &[InstantMillis(10), InstantMillis(20)]);
        assert_eq!(
            table.responsiveness(),
            &[
                RouteResponsiveness::Responsive,
                RouteResponsiveness::Unresponsive
            ]
        );
        assert_eq!(table.receiving_interfaces(), &[iface(0xE1), iface(0xE2)]);
        assert_eq!(table.retentions(), &[retention(10), retention(20)]);
    }

    #[test]
    fn set_row_updates_route_fields_without_changing_destination_or_len() {
        let mut table: FixedArrayRouteTable<2> = FixedArrayRouteTable::default();
        table
            .push(
                dest(0xA1),
                evidence(1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        table
            .push(
                dest(0xB2),
                evidence(2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        table.set_row(
            0,
            row(7, 70, RouteResponsiveness::Unresponsive, iface(0xE9)),
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(0xA1), dest(0xB2)]);
        assert_eq!(table.hops(), &[7, 2]);
        assert_eq!(table.learned_at(), &[InstantMillis(70), InstantMillis(20)]);
        assert_eq!(
            table.responsiveness(),
            &[
                RouteResponsiveness::Unresponsive,
                RouteResponsiveness::Responsive
            ]
        );
        assert_eq!(table.receiving_interfaces(), &[iface(0xE9), iface(0xE2)]);
        assert_eq!(table.retentions(), &[retention(70), retention(20)]);
        assert_eq!(table.evidence_ids(), &[evidence(1), evidence(2)]);
    }

    #[test]
    fn index_of_finds_a_present_destination_and_misses_an_absent_one() {
        let mut table: FixedArrayRouteTable<3> = FixedArrayRouteTable::default();
        table
            .push(
                dest(0xA1),
                RouteEvidenceId::FIRST,
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        table
            .push(
                dest(0xB2),
                RouteEvidenceId::FIRST,
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        assert_eq!(table.index_of(&dest(0xB2)), Some(1));
        assert_eq!(table.index_of(&dest(0xA1)), Some(0));
        assert_eq!(table.index_of(&dest(0xFF)), None);
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut table: FixedArrayRouteTable<3> = FixedArrayRouteTable::default();
        table
            .push(
                dest(0xA1),
                evidence(1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        table
            .push(
                dest(0xB2),
                evidence(2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();
        table
            .push(
                dest(0xC3),
                evidence(3),
                row(3, 30, RouteResponsiveness::Unresponsive, iface(0xE3)),
            )
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
    fn swap_remove_of_the_last_row_just_shrinks() {
        let mut table: FixedArrayRouteTable<3> = FixedArrayRouteTable::default();
        table
            .push(
                dest(0xA1),
                RouteEvidenceId::FIRST,
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        table
            .push(
                dest(0xB2),
                RouteEvidenceId::FIRST,
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        table.swap_remove(1, table.len() - 1);

        assert_eq!(table.len(), 1);
        assert_eq!(table.destinations(), &[dest(0xA1)]);
        assert_eq!(table.hops(), &[1]);
    }

    #[test]
    fn zero_capacity_columns_reject_push_without_exposing_rows() {
        let mut table: FixedArrayRouteTable<0> = FixedArrayRouteTable::default();

        assert_eq!(
            table.push(
                dest(0xA1),
                RouteEvidenceId::FIRST,
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1))
            ),
            Err(TablePushError::TableFull)
        );
        assert_eq!(table.len(), 0);
        assert!(table.destinations().is_empty());
        assert!(table.hops().is_empty());
        assert!(table.learned_at().is_empty());
        assert!(table.responsiveness().is_empty());
        assert!(table.receiving_interfaces().is_empty());
    }
}
