use crate::identity::known::{KnownDestinationRecord, KnownDestinationTable};
use crate::identity::{IdentityPublicKeys, KnownDestinationRetentionState};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceAppDataError, AppDataHandle};
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, Default)]
pub struct NoKnownDestinationTable;

impl KnownDestinationTable for NoKnownDestinationTable {
    fn capacity(&self) -> usize {
        0
    }

    fn len(&self) -> usize {
        0
    }

    fn destinations(&self) -> &[DestinationHash] {
        &[]
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &[]
    }

    fn announced_at(&self) -> &[InstantMillis] {
        &[]
    }

    fn retention(&self) -> &[KnownDestinationRetentionState] {
        &[]
    }

    fn app_data_handles(&self) -> &[AppDataHandle] {
        &[]
    }

    fn set_row(&mut self, _: usize, _: KnownDestinationRecord) {}

    fn push(
        &mut self,
        _: DestinationHash,
        _: KnownDestinationRecord,
    ) -> Result<usize, TablePushError> {
        Err(TablePushError::TableFull)
    }

    fn swap_remove(&mut self, _: usize) {}
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoKnownDestinationAppData;

impl AnnounceAppData for NoKnownDestinationAppData {
    fn get(&self, _: AppDataHandle) -> &[u8] {
        &[]
    }

    fn insert(&mut self, _: &[u8]) -> Result<AppDataHandle, AnnounceAppDataError> {
        Err(AnnounceAppDataError::TooManyEntries)
    }

    fn replace(&mut self, _: AppDataHandle, _: &[u8]) -> Result<(), AnnounceAppDataError> {
        Err(AnnounceAppDataError::TooManyEntries)
    }

    fn free(&mut self, _: AppDataHandle) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::known::KnownDestinations;

    #[test]
    fn disabled_known_destination_storage_is_zero_sized() {
        assert_eq!(core::mem::size_of::<NoKnownDestinationTable>(), 0);
        assert_eq!(core::mem::size_of::<NoKnownDestinationAppData>(), 0);
        assert_eq!(
            core::mem::size_of::<
                KnownDestinations<NoKnownDestinationTable, NoKnownDestinationAppData>,
            >(),
            0,
        );
    }
}
