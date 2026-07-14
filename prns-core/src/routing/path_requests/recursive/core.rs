use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;
/// RNS 1.3.5 `Transport.PATH_REQUEST_TIMEOUT` (15s)
pub const RECURSIVE_PATH_REQUEST_TIMEOUT_MS: u64 = 15_000;

pub trait RecursivePathRequestTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn requesting_interfaces(&self) -> &[InterfaceId];
    fn expires_ats(&self) -> &[InstantMillis];

    fn push(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    );
    fn swap_remove(&mut self, index: usize);

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn earliest_indexed_expiry(&mut self) -> Option<InstantMillis> {
        self.expires_ats()
            .iter()
            .copied()
            .min_by_key(|expires_at| expires_at.0)
    }

    fn first_expired(&mut self, now: InstantMillis) -> Option<usize> {
        self.expires_ats()
            .iter()
            .position(|expires_at| expires_at.0 <= now.0)
    }
}

/// RNS 1.3.5 `Transport.discovery_path_requests`.
#[derive(Debug, Default)]
pub struct RecursivePathRequests<C: RecursivePathRequestTable> {
    table: C,
    earliest_expiry: Option<InstantMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursiveOutcome {
    Opened,
    AlreadyInFlight,
}

impl<C: RecursivePathRequestTable> RecursivePathRequests<C> {
    pub fn begin(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    ) -> RecursiveOutcome {
        if self.index_of(&destination).is_some() {
            return RecursiveOutcome::AlreadyInFlight;
        }
        if self.table.len() >= self.table.capacity() {
            self.evict_soonest_expiring();
        }
        self.table
            .push(destination, requesting_interface, expires_at);
        self.refresh_earliest_expiry();
        RecursiveOutcome::Opened
    }

    pub fn take_requester(&mut self, destination: &DestinationHash) -> Option<InterfaceId> {
        let index = self.index_of(destination)?;
        let requesting_interface = self.table.requesting_interfaces()[index];
        self.table.swap_remove(index);
        self.refresh_earliest_expiry();
        Some(requesting_interface)
    }

    /// Whether a recursive path request is in flight for `destination`; like a pending request, it exempts the destination from ingress limiting.
    pub fn contains(&self, destination: &DestinationHash) -> bool {
        self.index_of(destination).is_some()
    }

    pub fn cull_expired(&mut self, now: InstantMillis) {
        while let Some(index) = self.table.first_expired(now) {
            self.table.swap_remove(index);
        }
        self.refresh_earliest_expiry();
    }

    fn refresh_earliest_expiry(&mut self) {
        self.earliest_expiry = self.table.earliest_indexed_expiry();
    }

    pub fn earliest_expiry_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_expiry,
            self.table
                .expires_ats()
                .iter()
                .copied()
                .min_by_key(|expires_at| expires_at.0),
            "earliest_expiry cache desynced from the expires_ats column"
        );
        self.earliest_expiry
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.table.index_of(destination)
    }

    fn evict_soonest_expiring(&mut self) {
        if let Some(index) = self
            .table
            .expires_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, expires_at)| expires_at.0)
            .map(|(index, _)| index)
        {
            self.table.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::path_requests::recursive::{
        FixedRecursivePathRequestTable, HeapRecursivePathRequestTable,
    };

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn asker(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn table() -> RecursivePathRequests<FixedRecursivePathRequestTable<4>> {
        RecursivePathRequests::default()
    }

    #[test]
    fn begin_is_idempotent_per_destination() {
        let mut table = table();
        assert_eq!(
            table.begin(dest(1), asker(0xA), InstantMillis(15_000)),
            RecursiveOutcome::Opened
        );
        assert_eq!(
            table.begin(dest(1), asker(0xB), InstantMillis(20_000)),
            RecursiveOutcome::AlreadyInFlight
        );
        assert_eq!(
            table.begin(dest(2), asker(0xC), InstantMillis(15_000)),
            RecursiveOutcome::Opened
        );
    }

    #[test]
    fn take_returns_the_asking_interface_then_retires_the_entry() {
        let mut table = table();
        table.begin(dest(1), asker(0xA), InstantMillis(15_000));
        assert_eq!(table.take_requester(&dest(1)), Some(asker(0xA)));
        assert_eq!(table.take_requester(&dest(1)), None);
        assert_eq!(
            table.begin(dest(1), asker(0xB), InstantMillis(30_000)),
            RecursiveOutcome::Opened
        );
    }

    #[test]
    fn cull_drops_only_entries_past_their_window() {
        let mut table = table();
        table.begin(dest(1), asker(0xA), InstantMillis(10_000));
        table.begin(dest(2), asker(0xB), InstantMillis(20_000));
        table.cull_expired(InstantMillis(15_000));
        assert_eq!(table.take_requester(&dest(1)), None);
        assert_eq!(table.take_requester(&dest(2)), Some(asker(0xB)));
    }

    #[test]
    fn earliest_expiry_is_the_soonest_window_close() {
        let mut table = table();
        assert_eq!(table.earliest_expiry_at(), None);
        table.begin(dest(1), asker(0xA), InstantMillis(20_000));
        table.begin(dest(2), asker(0xB), InstantMillis(12_000));
        assert_eq!(table.earliest_expiry_at(), Some(InstantMillis(12_000)));
    }

    #[test]
    fn a_full_table_evicts_its_soonest_expiring_entry() {
        let mut table = table();
        for (id, expiry) in [(1u8, 40_000u64), (2, 10_000), (3, 30_000), (4, 20_000)] {
            assert_eq!(
                table.begin(dest(id), asker(id), InstantMillis(expiry)),
                RecursiveOutcome::Opened
            );
        }
        assert_eq!(
            table.begin(dest(5), asker(5), InstantMillis(50_000)),
            RecursiveOutcome::Opened
        );
        assert_eq!(table.take_requester(&dest(2)), None);
        assert_eq!(table.take_requester(&dest(5)), Some(asker(5)));
    }

    #[cfg(feature = "std")]
    #[test]
    fn heap_indexes_destinations_and_expiries_across_row_moves() {
        let mut table: RecursivePathRequests<HeapRecursivePathRequestTable> =
            RecursivePathRequests::default();
        table.begin(dest(1), asker(1), InstantMillis(30_000));
        table.begin(dest(2), asker(2), InstantMillis(10_000));
        table.begin(dest(3), asker(3), InstantMillis(20_000));

        assert_eq!(table.earliest_expiry_at(), Some(InstantMillis(10_000)));
        assert_eq!(table.take_requester(&dest(1)), Some(asker(1)));
        table.cull_expired(InstantMillis(10_000));
        assert_eq!(table.take_requester(&dest(2)), None);
        assert_eq!(table.take_requester(&dest(3)), Some(asker(3)));
    }
}
