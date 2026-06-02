//! Heap-backed, growable routing-table columns (the typical std/alloc backend).
//!
//! The same SoA shape as [`FixedArrayRouteColumns`](super::FixedArrayRouteColumns),
//! one `Vec` per column joined by slot index, but with no ceiling: the table grows
//! with the network. `capacity()` is `usize::MAX`, so the engine's drop-when-full
//! check (`len >= capacity`) never trips, and `push` cannot fail.

use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::storage::{ColumnsFull, RouteColumns, RouteEntry};
use crate::routing::RouteResponsiveness;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRouteColumns {
    destination: Vec<DestinationHash>,
    hops: Vec<u8>,
    expires: Vec<InstantMillis>,
    responsiveness: Vec<RouteResponsiveness>,
    receiving_interface: Vec<InterfaceId>,
}

impl RouteColumns for HeapRouteColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destination.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destination
    }
    fn hops(&self) -> &[u8] {
        &self.hops
    }
    fn expires(&self) -> &[InstantMillis] {
        &self.expires
    }
    fn responsiveness(&self) -> &[RouteResponsiveness] {
        &self.responsiveness
    }
    fn receiving_interfaces(&self) -> &[InterfaceId] {
        &self.receiving_interface
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
        let i = self.destination.len();
        self.destination.push(destination);
        self.hops.push(row.hops);
        self.expires.push(row.expires);
        self.responsiveness.push(row.responsiveness);
        self.receiving_interface.push(row.receiving_interface);
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
    fn row(hops: u8, expires: u64, receiving_interface: InterfaceId) -> RouteEntry {
        RouteEntry {
            hops,
            expires: InstantMillis(expires),
            responsiveness: RouteResponsiveness::Responsive,
            receiving_interface,
        }
    }

    #[test]
    fn grows_past_any_fixed_ceiling_and_exposes_only_pushed_rows() {
        let mut columns = HeapRouteColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        assert!(columns.is_empty());

        for n in 0..1_000u32 {
            let byte = n as u8;
            assert_eq!(
                columns.push(dest(byte), row(1, n as u64, iface(byte))),
                Ok(n as usize)
            );
        }
        assert_eq!(columns.len(), 1_000);
        assert_eq!(columns.destinations().len(), 1_000);

        // set_row overwrites the route fields of a slot, leaving destination + len.
        columns.set_row(0, row(9, 99, iface(0xEE)));
        assert_eq!(columns.hops()[0], 9);
        assert_eq!(columns.receiving_interfaces()[0], iface(0xEE));
        assert_eq!(columns.destinations()[0], dest(0));
    }
}
