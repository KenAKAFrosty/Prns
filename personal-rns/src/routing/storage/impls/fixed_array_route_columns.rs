//! Fixed-capacity, inline-array routing-table columns — the no_std default.
//!
//! Each column is a `[T; MAX_TRACKED_DESTINATIONS]` stored inline in the
//! struct (and therefore inline in whatever `RoutingTable` it lives in).
//! No allocator, no heap, no growth: footprint is known at compile time and
//! sized by the const generic. Capacity overflow surfaces as
//! [`ColumnsFull`] at the `push` call site.

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::storage::{ColumnsFull, RouteColumns, RouteEntry};
use crate::routing::RouteResponsiveness;
use crate::wire::DestinationHash;

/// SoA routing-table columns backed by inline fixed-size arrays. The
/// capacity is the const generic; reaching it returns `ColumnsFull` from
/// `push`.
///
/// `PartialEq` is structural — every slot compares, including unused tail
/// past `len`. Determinism tests rely on this exactly as `RoutingTable`
/// already does; it is not "same set of destinations."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayRouteColumns<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    expires: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [RouteResponsiveness; MAX_TRACKED_DESTINATIONS],
    receiving_interface: [InterfaceId; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            expires: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [RouteResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            receiving_interface: [InterfaceId::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
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
    fn expires(&self) -> &[InstantMillis] {
        &self.expires[..self.len]
    }
    fn responsiveness(&self) -> &[RouteResponsiveness] {
        &self.responsiveness[..self.len]
    }
    fn receiving_interfaces(&self) -> &[InterfaceId] {
        &self.receiving_interface[..self.len]
    }

    fn set_row(&mut self, i: usize, row: RouteEntry) {
        self.hops[i] = row.hops;
        self.expires[i] = row.expires;
        self.responsiveness[i] = row.responsiveness;
        self.receiving_interface[i] = row.receiving_interface;
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

    fn row(
        hops: u8,
        expires: u64,
        responsiveness: RouteResponsiveness,
        receiving_interface: InterfaceId,
    ) -> RouteEntry {
        RouteEntry {
            hops,
            expires: InstantMillis(expires),
            responsiveness,
            receiving_interface,
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
        assert_eq!(columns.expires(), &[InstantMillis(10), InstantMillis(20)]);
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
        assert_eq!(columns.expires(), &[InstantMillis(70), InstantMillis(20)]);
        assert_eq!(
            columns.responsiveness(),
            &[
                RouteResponsiveness::Unresponsive,
                RouteResponsiveness::Responsive
            ]
        );
        // set_row rewrites the receiving interface alongside the other fields.
        assert_eq!(columns.receiving_interfaces(), &[iface(0xE9), iface(0xE2)]);
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
        assert!(columns.expires().is_empty());
        assert!(columns.responsiveness().is_empty());
        assert!(columns.receiving_interfaces().is_empty());
    }
}
