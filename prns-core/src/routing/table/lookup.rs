use super::model::{ExistingRoute, ForwardingRoute, StoredAnnounce};
use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::announce::Announce;
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{
    RouteEntry, RouteEvidenceHandle, RouteEvidenceId, RouteResponsiveness, RouteTable,
};
use crate::routing::warmth::RouteWarmth;
use crate::wire::DestinationHash;

fn route_evidence_row_hint(row: usize) -> u16 {
    u16::try_from(row).unwrap_or(u16::MAX)
}

fn route_evidence_scan_start(row_hint: u16, len: usize) -> usize {
    debug_assert!(len > 0);
    if row_hint == u16::MAX {
        len - 1
    } else {
        usize::from(row_hint).min(len - 1)
    }
}

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.stored_announce_for(destination)?.announce.app_data)
    }

    pub fn stored_announce_for(&self, destination: &DestinationHash) -> Option<StoredAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.announce_records.app_data_handles()[i]?;
        let app_data = self.announce_app_data.get(handle);
        Some(StoredAnnounce {
            hops: self.routes.hops()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            announce: Announce {
                destination: self.routes.destinations()[i],
                public_keys: self.announce_records.public_keys()[i],
                dotted_name_hash: self.announce_records.dotted_name_hashes()[i],
                announce_id: self.announce_records.announce_ids()[i],
                ratchet: self.announce_records.ratchets()[i],
                signature: self.announce_records.signatures()[i],
                app_data,
            },
        })
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routes.route_count_via(interface)
    }

    pub fn hop_count_to(&self, destination: &DestinationHash) -> Option<u8> {
        self.index_of(destination).map(|i| self.routes.hops()[i])
    }

    pub fn has_route(&self, destination: &DestinationHash) -> bool {
        self.index_of(destination).is_some()
    }

    pub fn route_evidence_handle_for(
        &self,
        destination: &DestinationHash,
    ) -> Option<RouteEvidenceHandle> {
        let row = self.index_of(destination)?;
        // Fixed profiles fit exactly. A very large heap table uses MAX as an overflow sentinel;
        // resolution then starts at the tail and remains correct without widening the handle.
        let row_hint = route_evidence_row_hint(row);
        Some(RouteEvidenceHandle::new(
            self.routes.evidence_ids()[row],
            row_hint,
        ))
    }

    /// Resolves the authoritative id, repairing a row hint made stale by swap-removal.
    ///
    /// A surviving route can only remain in place or move downward: insertions append and every
    /// removal moves the last row into a lower hole. No upward or wraparound scan is needed.
    pub(crate) fn resolve_route_evidence(&self, handle: &mut RouteEvidenceHandle) -> Option<usize> {
        let len = self.routes.len();
        if len == 0 {
            return None;
        }
        let start = route_evidence_scan_start(handle.row_hint, len);
        for row in (0..=start).rev() {
            if self.routes.evidence_ids()[row] == handle.id {
                handle.row_hint = route_evidence_row_hint(row);
                return Some(row);
            }
        }
        None
    }

    pub(crate) fn route_evidence_ids(&self) -> &[RouteEvidenceId] {
        self.routes.evidence_ids()
    }

    pub fn responsiveness_of(&self, destination: &DestinationHash) -> Option<RouteResponsiveness> {
        self.index_of(destination)
            .map(|i| self.routes.responsiveness()[i])
    }

    pub fn path_rows(&self) -> impl Iterator<Item = (DestinationHash, RouteEntry)> + '_ {
        let routes = &self.routes;
        (0..routes.len()).map(move |i| (routes.destinations()[i], self.path_row_at(i)))
    }

    pub(crate) fn path_rows_with_expiry<'a>(
        &'a self,
        interfaces: AttachedInterfaces<'a>,
        warmth: &'a dyn RouteWarmth,
    ) -> impl Iterator<Item = (DestinationHash, RouteEntry, InstantMillis)> + 'a {
        let routes = &self.routes;
        (0..routes.len()).map(move |i| {
            (
                routes.destinations()[i],
                self.path_row_at(i),
                self.expiry_of_with_warmth(i, interfaces, warmth),
            )
        })
    }

    pub fn path_row(&self, destination: &DestinationHash) -> Option<RouteEntry> {
        let i = self.index_of(destination)?;
        Some(self.path_row_at(i))
    }

    pub(crate) fn path_row_with_expiry(
        &self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> Option<(RouteEntry, InstantMillis)> {
        let i = self.index_of(destination)?;
        Some((
            self.path_row_at(i),
            self.expiry_of_with_warmth(i, interfaces, warmth),
        ))
    }

    pub(super) fn path_row_at(&self, i: usize) -> RouteEntry {
        RouteEntry {
            hops: self.routes.hops()[i],
            learned_at: self.routes.learned_at()[i],
            responsiveness: self.routes.responsiveness()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            last_route_activity_at: self.routes.last_route_activity_at()[i],
        }
    }

    pub(super) fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.routes.index_of(destination)
    }

    pub fn existing_route_for(
        &self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
    ) -> Option<ExistingRoute<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            expires_at: self.gate_expiry_of(i, interfaces),
            announce_id_history: self.announce_id_history.history(i),
            responsiveness: self.routes.responsiveness()[i],
            interface_gravity: interfaces
                .descriptor_for(self.routes.receiving_interfaces()[i])
                .map(|descriptor| descriptor.gravity),
        })
    }

    pub fn forwarding_route_for(&self, destination: &DestinationHash) -> Option<ForwardingRoute> {
        let i = self.index_of(destination)?;
        Some(ForwardingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{route_evidence_row_hint, route_evidence_scan_start};

    #[test]
    fn oversized_heap_rows_use_the_tail_as_their_compact_hint() {
        assert_eq!(route_evidence_row_hint(7), 7);
        assert_eq!(route_evidence_scan_start(7, 20), 7);

        let oversized_row = usize::from(u16::MAX) + 9;
        assert_eq!(route_evidence_row_hint(oversized_row), u16::MAX);
        assert_eq!(
            route_evidence_scan_start(u16::MAX, oversized_row + 5),
            oversized_row + 4,
        );
    }
}
