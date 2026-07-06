use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::routes::{RouteColumns, RouteEntry};
use crate::routing::{NextHop, RouteResponsiveness};
use crate::storage::ColumnsFull;
use crate::wire::DestinationHash;

/// `PartialEq` is structural — every slot compares, including unused tail
/// past `len`. Determinism tests rely on this exactly as `RoutingTable`
/// already does; it is not "same set of destinations."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayRouteColumns<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    learned_at: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    last_relayed_at: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [RouteResponsiveness; MAX_TRACKED_DESTINATIONS],
    receiving_interface: [InterfaceId; MAX_TRACKED_DESTINATIONS],
    next_hop: [NextHop; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            learned_at: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            last_relayed_at: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [RouteResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            receiving_interface: [InterfaceId::new([0u8; 8]); MAX_TRACKED_DESTINATIONS],
            next_hop: [NextHop::Direct; MAX_TRACKED_DESTINATIONS],
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize> RouteColumns
    for FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>
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
    fn last_relayed_at(&self) -> &[InstantMillis] {
        &self.last_relayed_at[..self.len]
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
    ) -> Result<usize, ColumnsFull> {
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(ColumnsFull);
        }
        let i = self.len;
        self.destination[i] = destination;
        self.set_row(i, row);
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.len - 1);
        self.destination[i] = self.destination[last];
        self.hops[i] = self.hops[last];
        self.learned_at[i] = self.learned_at[last];
        self.last_relayed_at[i] = self.last_relayed_at[last];
        self.responsiveness[i] = self.responsiveness[last];
        self.receiving_interface[i] = self.receiving_interface[last];
        self.next_hop[i] = self.next_hop[last];
        self.len = last;
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

    fn row(
        hops: u8,
        learned_at: u64,
        responsiveness: RouteResponsiveness,
        receiving_interface: InterfaceId,
    ) -> RouteEntry {
        RouteEntry {
            hops,
            learned_at: InstantMillis(learned_at),
            last_relayed_at: InstantMillis(0),
            responsiveness,
            receiving_interface,
            next_hop: NextHop::Direct,
        }
    }

    #[test]
    fn push_exposes_only_initialized_rows() {
        let mut columns: FixedArrayRouteColumns<3> = FixedArrayRouteColumns::default();

        assert_eq!(columns.capacity(), 3);
        assert!(columns.is_empty());
        assert_eq!(
            columns.push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1))
            ),
            Ok(0)
        );
        assert_eq!(
            columns.push(
                dest(0xB2),
                row(2, 20, RouteResponsiveness::Unresponsive, iface(0xE2))
            ),
            Ok(1)
        );

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.destinations(), &[dest(0xA1), dest(0xB2)]);
        assert_eq!(columns.hops(), &[1, 2]);
        assert_eq!(
            columns.learned_at(),
            &[InstantMillis(10), InstantMillis(20)]
        );
        assert_eq!(
            columns.responsiveness(),
            &[
                RouteResponsiveness::Responsive,
                RouteResponsiveness::Unresponsive
            ]
        );
        assert_eq!(columns.receiving_interfaces(), &[iface(0xE1), iface(0xE2)]);
    }

    #[test]
    fn set_row_updates_route_fields_without_changing_destination_or_len() {
        let mut columns: FixedArrayRouteColumns<2> = FixedArrayRouteColumns::default();
        columns
            .push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        columns
            .push(
                dest(0xB2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        columns.set_row(
            0,
            row(7, 70, RouteResponsiveness::Unresponsive, iface(0xE9)),
        );

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.destinations(), &[dest(0xA1), dest(0xB2)]);
        assert_eq!(columns.hops(), &[7, 2]);
        assert_eq!(
            columns.learned_at(),
            &[InstantMillis(70), InstantMillis(20)]
        );
        assert_eq!(
            columns.responsiveness(),
            &[
                RouteResponsiveness::Unresponsive,
                RouteResponsiveness::Responsive
            ]
        );
        assert_eq!(columns.receiving_interfaces(), &[iface(0xE9), iface(0xE2)]);
    }

    #[test]
    fn index_of_finds_a_present_destination_and_misses_an_absent_one() {
        let mut columns: FixedArrayRouteColumns<3> = FixedArrayRouteColumns::default();
        columns
            .push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        columns
            .push(
                dest(0xB2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        assert_eq!(columns.index_of(&dest(0xB2)), Some(1));
        assert_eq!(columns.index_of(&dest(0xA1)), Some(0));
        assert_eq!(columns.index_of(&dest(0xFF)), None);
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut columns: FixedArrayRouteColumns<3> = FixedArrayRouteColumns::default();
        columns
            .push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        columns
            .push(
                dest(0xB2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();
        columns
            .push(
                dest(0xC3),
                row(3, 30, RouteResponsiveness::Unresponsive, iface(0xE3)),
            )
            .unwrap();

        columns.swap_remove(0, columns.len() - 1);

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
    fn swap_remove_of_the_last_row_just_shrinks() {
        let mut columns: FixedArrayRouteColumns<3> = FixedArrayRouteColumns::default();
        columns
            .push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1)),
            )
            .unwrap();
        columns
            .push(
                dest(0xB2),
                row(2, 20, RouteResponsiveness::Responsive, iface(0xE2)),
            )
            .unwrap();

        columns.swap_remove(1, columns.len() - 1);

        assert_eq!(columns.len(), 1);
        assert_eq!(columns.destinations(), &[dest(0xA1)]);
        assert_eq!(columns.hops(), &[1]);
    }

    #[test]
    fn zero_capacity_columns_reject_push_without_exposing_rows() {
        let mut columns: FixedArrayRouteColumns<0> = FixedArrayRouteColumns::default();

        assert_eq!(
            columns.push(
                dest(0xA1),
                row(1, 10, RouteResponsiveness::Responsive, iface(0xE1))
            ),
            Err(ColumnsFull)
        );
        assert_eq!(columns.len(), 0);
        assert!(columns.destinations().is_empty());
        assert!(columns.hops().is_empty());
        assert!(columns.learned_at().is_empty());
        assert!(columns.responsiveness().is_empty());
        assert!(columns.receiving_interfaces().is_empty());
    }
}
