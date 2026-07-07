mod impls;

pub use impls::*;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;
/// RNS 1.3.5 `Transport.PATH_REQUEST_TIMEOUT` (15s)
pub const RECURSIVE_PATH_REQUEST_TIMEOUT_MS: u64 = 15_000;

pub trait RecursivePathRequestColumns {
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
}

/// RNS `discovery_path_requests`.
#[derive(Debug, Default)]
pub struct RecursivePathRequests<C: RecursivePathRequestColumns> {
    columns: C,
    earliest_expiry: Option<InstantMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursiveOutcome {
    Opened,
    AlreadyInFlight,
}

impl<C: RecursivePathRequestColumns> RecursivePathRequests<C> {
    pub fn begin(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    ) -> RecursiveOutcome {
        if self.index_of(&destination).is_some() {
            return RecursiveOutcome::AlreadyInFlight;
        }
        if self.columns.len() >= self.columns.capacity() {
            self.evict_soonest_expiring();
        }
        self.columns
            .push(destination, requesting_interface, expires_at);
        self.refresh_earliest_expiry();
        RecursiveOutcome::Opened
    }

    pub fn take_requester(&mut self, destination: &DestinationHash) -> Option<InterfaceId> {
        let index = self.index_of(destination)?;
        let requesting_interface = self.columns.requesting_interfaces()[index];
        self.columns.swap_remove(index);
        self.refresh_earliest_expiry();
        Some(requesting_interface)
    }

    /// Whether a recursive path request is in flight for `destination`; like a pending
    /// request, it exempts the destination from ingress limiting.
    pub fn contains(&self, destination: &DestinationHash) -> bool {
        self.index_of(destination).is_some()
    }

    pub fn cull_expired(&mut self, now: InstantMillis) {
        while let Some(index) = self
            .columns
            .expires_ats()
            .iter()
            .position(|expires_at| expires_at.0 <= now.0)
        {
            self.columns.swap_remove(index);
        }
        self.refresh_earliest_expiry();
    }

    fn refresh_earliest_expiry(&mut self) {
        self.earliest_expiry = self
            .columns
            .expires_ats()
            .iter()
            .copied()
            .min_by_key(|expires_at| expires_at.0);
    }

    pub fn earliest_expiry_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_expiry,
            self.columns
                .expires_ats()
                .iter()
                .copied()
                .min_by_key(|expires_at| expires_at.0),
            "earliest_expiry cache desynced from the expires_ats column"
        );
        self.earliest_expiry
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn evict_soonest_expiring(&mut self) {
        if let Some(index) = self
            .columns
            .expires_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, expires_at)| expires_at.0)
            .map(|(index, _)| index)
        {
            self.columns.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::path_requests::recursive::FixedRecursivePathRequestColumns;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn asker(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn table() -> RecursivePathRequests<FixedRecursivePathRequestColumns<4>> {
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
}
