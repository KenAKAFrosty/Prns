//! Fixed-capacity, inline-array routing-table columns — the no_std default.
//!
//! Each column is a `[T; MAX_TRACKED_DESTINATIONS]` stored inline in the
//! struct (and therefore inline in whatever `RoutingTable` it lives in).
//! No allocator, no heap, no growth: footprint is known at compile time and
//! sized by the const generic. Capacity overflow surfaces as
//! [`ColumnsFull`](crate::routing::storage::ColumnsFull) at the `push` call site.

use crate::engine::InstantMillis;
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

    fn set_row(&mut self, i: usize, row: RouteEntry) {
        self.hops[i] = row.hops;
        self.expires[i] = row.expires;
        self.responsiveness[i] = row.responsiveness;
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
