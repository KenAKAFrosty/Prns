use core::num::NonZeroU32;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::buckets_for_two_thirds_load;
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TransportId};

pub const fn route_index_buckets(destinations: usize) -> usize {
    buckets_for_two_thirds_load(destinations)
}

/// Process-local identity for one live incarnation of a routing-table path.
///
/// The value is attribution, not a clock: unrelated route changes never invalidate it. A removed
/// or materially replaced path retires its value, so later evidence cannot refresh a newer route
/// merely because the destination hash is the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RouteEvidenceId(NonZeroU32);

impl RouteEvidenceId {
    pub const FIRST: Self = Self(NonZeroU32::MIN);

    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub(super) const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Compact, validated locator retained by traffic that can later prove a route worked.
///
/// `row_hint` is only a fast path. The evidence id is authoritative and survives swap-removal
/// moving its route toward row zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct RouteEvidenceHandle {
    pub id: RouteEvidenceId,
    pub row_hint: u16,
}

impl RouteEvidenceHandle {
    pub const fn new(id: RouteEvidenceId, row_hint: u16) -> Self {
        Self { id, row_hint }
    }
}

const _: () = {
    assert!(core::mem::size_of::<RouteEvidenceId>() == 4);
    assert!(core::mem::size_of::<Option<RouteEvidenceId>>() == 4);
    assert!(core::mem::size_of::<RouteEvidenceHandle>() == 8);
    assert!(core::mem::size_of::<Option<RouteEvidenceHandle>>() == 8);
};

/// RNS 1.4.2 `Transport.path_table`'s `received_from` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextHop {
    Direct,
    Via(TransportId),
}

/// RNS 1.4.2 `Transport.path_is_unresponsive`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResponsiveness {
    Unknown,
    Responsive,
    Unresponsive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub hops: u8,
    pub learned_at: InstantMillis,
    pub last_route_activity_at: InstantMillis,
    pub responsiveness: RouteResponsiveness,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

pub trait RouteTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.receiving_interfaces()
            .iter()
            .filter(|&&candidate| candidate == interface)
            .count()
    }

    fn repoint_receiving_interface(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
    ) -> usize {
        let mut moved = 0;
        for row in 0..self.len() {
            if self.receiving_interfaces()[row] != previous {
                continue;
            }
            self.set_row(
                row,
                RouteEntry {
                    hops: self.hops()[row],
                    learned_at: self.learned_at()[row],
                    last_route_activity_at: now,
                    responsiveness: self.responsiveness()[row],
                    receiving_interface: current,
                    next_hop: self.next_hops()[row],
                },
            );
            moved += 1;
        }
        moved
    }

    fn destinations(&self) -> &[DestinationHash];
    fn hops(&self) -> &[u8];
    fn learned_at(&self) -> &[InstantMillis];
    fn last_route_activity_at(&self) -> &[InstantMillis];
    fn responsiveness(&self) -> &[RouteResponsiveness];
    fn receiving_interfaces(&self) -> &[InterfaceId];
    fn next_hops(&self) -> &[NextHop];
    fn evidence_ids(&self) -> &[RouteEvidenceId];

    fn set_row(&mut self, i: usize, row: RouteEntry);
    fn set_evidence_id(&mut self, i: usize, evidence_id: RouteEvidenceId);

    fn push(
        &mut self,
        destination: DestinationHash,
        evidence_id: RouteEvidenceId,
        row: RouteEntry,
    ) -> Result<usize, TablePushError>;

    fn swap_remove(&mut self, i: usize, last: usize);
}
