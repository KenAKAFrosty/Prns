use crate::identity::destination_identity::{DestinationIdentityRecord, DestinationIdentityTable};
use crate::identity::{DestinationIdentityRetentionState, IdentityPublicKeys};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceAppDataError, AppDataHandle};
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, Default)]
pub struct NoDestinationIdentityTable;

impl DestinationIdentityTable for NoDestinationIdentityTable {
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

    fn retention(&self) -> &[DestinationIdentityRetentionState] {
        &[]
    }

    fn app_data_handles(&self) -> &[AppDataHandle] {
        &[]
    }

    fn set_row(&mut self, _: usize, _: DestinationIdentityRecord) {}

    fn push(
        &mut self,
        _: DestinationHash,
        _: DestinationIdentityRecord,
    ) -> Result<usize, TablePushError> {
        Err(TablePushError::TableFull)
    }

    fn swap_remove(&mut self, _: usize) {}
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoDestinationIdentityAppData;

impl AnnounceAppData for NoDestinationIdentityAppData {
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
    use crate::identity::destination_identity::DestinationIdentities;

    #[test]
    fn disabled_destination_identity_storage_is_zero_sized() {
        assert_eq!(core::mem::size_of::<NoDestinationIdentityTable>(), 0);
        assert_eq!(core::mem::size_of::<NoDestinationIdentityAppData>(), 0);
        assert_eq!(
            core::mem::size_of::<
                DestinationIdentities<NoDestinationIdentityTable, NoDestinationIdentityAppData>,
            >(),
            0,
        );
    }
}
