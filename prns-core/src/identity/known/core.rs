use crate::identity::{IdentityHash, IdentityPublicKeys};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceAppDataError, AppDataHandle};
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

pub const UNUSED_DESTINATION_LINGER_MILLIS: u64 = 6 * 60 * 1_000;
pub const USED_DESTINATION_LINGER_MILLIS: u64 = 7 * 24 * 60 * 60 * 1_000 * 5 / 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownDestinationRetentionState {
    NeverUsed,
    UsedAt(InstantMillis),
    Retained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDestinationRecord {
    pub public_keys: IdentityPublicKeys,
    pub announced_at: InstantMillis,
    pub retention: KnownDestinationRetentionState,
    pub app_data_handle: AppDataHandle,
}

pub trait KnownDestinationTable {
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

    fn destinations(&self) -> &[DestinationHash];
    fn public_keys(&self) -> &[IdentityPublicKeys];
    fn announced_at(&self) -> &[InstantMillis];
    fn retention(&self) -> &[KnownDestinationRetentionState];
    fn app_data_handles(&self) -> &[AppDataHandle];
    fn set_row(&mut self, index: usize, record: KnownDestinationRecord);
    fn push(
        &mut self,
        destination: DestinationHash,
        record: KnownDestinationRecord,
    ) -> Result<usize, TablePushError>;
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDestination<'a> {
    pub destination: DestinationHash,
    pub identity: IdentityHash,
    pub public_keys: IdentityPublicKeys,
    pub announced_at: InstantMillis,
    pub retention: KnownDestinationRetentionState,
    pub app_data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberKnownDestinationOutcome {
    Remembered,
    Refreshed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberKnownDestinationError {
    PublicKeyChanged,
    TableFull,
    AppDataFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkDestinationUsedOutcome {
    Recorded,
    Refreshed,
    Retained,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainDestinationOutcome {
    Retained,
    AlreadyRetained,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDestinationOutcome {
    Released,
    UseRecorded,
    UseRefreshed,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainIdentityOutcome {
    pub newly_retained_destination_count: u32,
    pub already_retained_destination_count: u32,
}

pub struct KnownDestinations<Table, AppData> {
    table: Table,
    app_data: AppData,
}

impl<Table: Default, AppData: Default> Default for KnownDestinations<Table, AppData> {
    fn default() -> Self {
        Self {
            table: Table::default(),
            app_data: AppData::default(),
        }
    }
}

impl<Table: KnownDestinationTable, AppData: AnnounceAppData> KnownDestinations<Table, AppData> {
    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn contains(&self, destination: &DestinationHash) -> bool {
        self.table.index_of(destination).is_some()
    }

    pub fn get(&self, destination: &DestinationHash) -> Option<KnownDestination<'_>> {
        let index = self.table.index_of(destination)?;
        Some(self.row_at(index))
    }

    pub fn rows(&self) -> impl Iterator<Item = KnownDestination<'_>> + '_ {
        (0..self.table.len()).map(|index| self.row_at(index))
    }

    pub fn remember(
        &mut self,
        destination: DestinationHash,
        public_keys: IdentityPublicKeys,
        app_data: &[u8],
        announced_at: InstantMillis,
    ) -> Result<RememberKnownDestinationOutcome, RememberKnownDestinationError> {
        match self.table.index_of(&destination) {
            Some(index) => {
                if self.table.public_keys()[index] != public_keys {
                    return Err(RememberKnownDestinationError::PublicKeyChanged);
                }
                let app_data_handle = self.table.app_data_handles()[index];
                self.app_data
                    .replace(app_data_handle, app_data)
                    .map_err(classify_app_data_error)?;
                self.table.set_row(
                    index,
                    KnownDestinationRecord {
                        public_keys,
                        announced_at,
                        retention: self.table.retention()[index],
                        app_data_handle,
                    },
                );
                Ok(RememberKnownDestinationOutcome::Refreshed)
            }
            None => {
                let app_data_handle = self
                    .app_data
                    .insert(app_data)
                    .map_err(classify_app_data_error)?;
                let record = KnownDestinationRecord {
                    public_keys,
                    announced_at,
                    retention: KnownDestinationRetentionState::NeverUsed,
                    app_data_handle,
                };
                if self.table.push(destination, record).is_err() {
                    self.app_data.free(app_data_handle);
                    return Err(RememberKnownDestinationError::TableFull);
                }
                Ok(RememberKnownDestinationOutcome::Remembered)
            }
        }
    }

    pub fn mark_used(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> MarkDestinationUsedOutcome {
        let Some(index) = self.table.index_of(destination) else {
            return MarkDestinationUsedOutcome::NotFound;
        };
        match self.table.retention()[index] {
            KnownDestinationRetentionState::Retained => MarkDestinationUsedOutcome::Retained,
            KnownDestinationRetentionState::NeverUsed => {
                self.set_retention(index, KnownDestinationRetentionState::UsedAt(now));
                MarkDestinationUsedOutcome::Recorded
            }
            KnownDestinationRetentionState::UsedAt(_) => {
                self.set_retention(index, KnownDestinationRetentionState::UsedAt(now));
                MarkDestinationUsedOutcome::Refreshed
            }
        }
    }

    pub fn retain(&mut self, destination: &DestinationHash) -> RetainDestinationOutcome {
        let Some(index) = self.table.index_of(destination) else {
            return RetainDestinationOutcome::NotFound;
        };
        if self.table.retention()[index] == KnownDestinationRetentionState::Retained {
            return RetainDestinationOutcome::AlreadyRetained;
        }
        self.set_retention(index, KnownDestinationRetentionState::Retained);
        RetainDestinationOutcome::Retained
    }

    pub fn release(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> ReleaseDestinationOutcome {
        let Some(index) = self.table.index_of(destination) else {
            return ReleaseDestinationOutcome::NotFound;
        };
        let outcome = match self.table.retention()[index] {
            KnownDestinationRetentionState::Retained => ReleaseDestinationOutcome::Released,
            KnownDestinationRetentionState::NeverUsed => ReleaseDestinationOutcome::UseRecorded,
            KnownDestinationRetentionState::UsedAt(_) => ReleaseDestinationOutcome::UseRefreshed,
        };
        self.set_retention(index, KnownDestinationRetentionState::UsedAt(now));
        outcome
    }

    pub fn retain_identity(&mut self, identity: &IdentityHash) -> RetainIdentityOutcome {
        let mut outcome = RetainIdentityOutcome {
            newly_retained_destination_count: 0,
            already_retained_destination_count: 0,
        };
        for index in 0..self.table.len() {
            if self.table.public_keys()[index].identity_hash() != *identity {
                continue;
            }
            if self.table.retention()[index] == KnownDestinationRetentionState::Retained {
                outcome.already_retained_destination_count =
                    outcome.already_retained_destination_count.saturating_add(1);
            } else {
                self.set_retention(index, KnownDestinationRetentionState::Retained);
                outcome.newly_retained_destination_count =
                    outcome.newly_retained_destination_count.saturating_add(1);
            }
        }
        outcome
    }

    pub fn cull_expired(
        &mut self,
        now: InstantMillis,
        mut has_path: impl FnMut(&DestinationHash) -> bool,
    ) -> usize {
        let mut removed = 0;
        let mut index = 0;
        while index < self.table.len() {
            if !has_path(&self.table.destinations()[index]) && self.expired_at(index, now) {
                self.remove(index);
                removed += 1;
            } else {
                index += 1;
            }
        }
        removed
    }

    pub fn evict_oldest_unretained_without_path(
        &mut self,
        mut has_path: impl FnMut(&DestinationHash) -> bool,
    ) -> bool {
        let candidate = (0..self.table.len())
            .filter(|&index| {
                self.table.retention()[index] != KnownDestinationRetentionState::Retained
                    && !has_path(&self.table.destinations()[index])
            })
            .min_by_key(|&index| self.expiry_at(index).unwrap_or(InstantMillis(u64::MAX)));
        match candidate {
            Some(index) => {
                self.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn soonest_expiry(
        &self,
        mut has_path: impl FnMut(&DestinationHash) -> bool,
    ) -> Option<InstantMillis> {
        (0..self.table.len())
            .filter(|&index| !has_path(&self.table.destinations()[index]))
            .filter_map(|index| self.expiry_at(index))
            .min()
    }

    fn row_at(&self, index: usize) -> KnownDestination<'_> {
        KnownDestination {
            destination: self.table.destinations()[index],
            identity: self.table.public_keys()[index].identity_hash(),
            public_keys: self.table.public_keys()[index],
            announced_at: self.table.announced_at()[index],
            retention: self.table.retention()[index],
            app_data: self.app_data.get(self.table.app_data_handles()[index]),
        }
    }

    fn set_retention(&mut self, index: usize, retention: KnownDestinationRetentionState) {
        self.table.set_row(
            index,
            KnownDestinationRecord {
                public_keys: self.table.public_keys()[index],
                announced_at: self.table.announced_at()[index],
                retention,
                app_data_handle: self.table.app_data_handles()[index],
            },
        );
    }

    fn expiry_anchor(&self, index: usize) -> InstantMillis {
        match self.table.retention()[index] {
            KnownDestinationRetentionState::NeverUsed => self.table.announced_at()[index],
            KnownDestinationRetentionState::UsedAt(used_at) => used_at,
            KnownDestinationRetentionState::Retained => InstantMillis(u64::MAX),
        }
    }

    fn expiry_at(&self, index: usize) -> Option<InstantMillis> {
        let linger = match self.table.retention()[index] {
            KnownDestinationRetentionState::NeverUsed => UNUSED_DESTINATION_LINGER_MILLIS,
            KnownDestinationRetentionState::UsedAt(_) => USED_DESTINATION_LINGER_MILLIS,
            KnownDestinationRetentionState::Retained => return None,
        };
        self.expiry_anchor(index)
            .0
            .checked_add(linger)?
            .checked_add(1)
            .map(InstantMillis)
    }

    fn expired_at(&self, index: usize, now: InstantMillis) -> bool {
        self.expiry_at(index).is_some_and(|expiry| now >= expiry)
    }

    fn remove(&mut self, index: usize) {
        let app_data_handle = self.table.app_data_handles()[index];
        self.app_data.free(app_data_handle);
        self.table.swap_remove(index);
    }
}

fn classify_app_data_error(_: AnnounceAppDataError) -> RememberKnownDestinationError {
    RememberKnownDestinationError::AppDataFull
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::known::{
        known_destination_index_buckets, FixedIndexedKnownDestinationTable,
    };
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::announce::stored::PackedAppDataArena;

    type Table = FixedIndexedKnownDestinationTable<4, { known_destination_index_buckets(4) }>;
    type Store = KnownDestinations<Table, PackedAppDataArena<64, 4>>;

    fn destination(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn keys(byte: u8) -> IdentityPublicKeys {
        IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([byte; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([byte; 32])),
        }
    }

    fn remember(
        store: &mut Store,
        destination_byte: u8,
        identity_byte: u8,
        app_data: &[u8],
        announced_at: u64,
    ) -> Result<RememberKnownDestinationOutcome, RememberKnownDestinationError> {
        store.remember(
            destination(destination_byte),
            keys(identity_byte),
            app_data,
            InstantMillis(announced_at),
        )
    }

    #[test]
    fn remember_and_refresh_keep_one_indexed_identity_record() {
        let mut store = Store::default();
        assert_eq!(
            remember(&mut store, 1, 0xA1, b"first", 1_000),
            Ok(RememberKnownDestinationOutcome::Remembered),
        );
        assert_eq!(
            store.retain(&destination(1)),
            RetainDestinationOutcome::Retained
        );
        assert_eq!(
            remember(&mut store, 1, 0xA1, b"second", 2_000),
            Ok(RememberKnownDestinationOutcome::Refreshed),
        );

        assert_eq!(store.len(), 1);
        assert_eq!(
            store.get(&destination(1)),
            Some(KnownDestination {
                destination: destination(1),
                identity: keys(0xA1).identity_hash(),
                public_keys: keys(0xA1),
                announced_at: InstantMillis(2_000),
                retention: KnownDestinationRetentionState::Retained,
                app_data: b"second",
            })
        );
    }

    #[test]
    fn a_public_key_change_is_refused_without_mutating_the_record() {
        let mut store = Store::default();
        remember(&mut store, 1, 0xA1, b"first", 1_000).unwrap();

        assert_eq!(
            remember(&mut store, 1, 0xB2, b"forged", 2_000),
            Err(RememberKnownDestinationError::PublicKeyChanged),
        );
        let retained = store.get(&destination(1)).unwrap();
        assert_eq!(retained.public_keys, keys(0xA1));
        assert_eq!(retained.announced_at, InstantMillis(1_000));
        assert_eq!(retained.app_data, b"first");
    }

    #[test]
    fn mark_used_matches_the_rns_retained_guard() {
        let mut store = Store::default();
        remember(&mut store, 1, 0xA1, b"", 1_000).unwrap();

        assert_eq!(
            store.mark_used(&destination(1), InstantMillis(2_000)),
            MarkDestinationUsedOutcome::Recorded,
        );
        assert_eq!(
            store.mark_used(&destination(1), InstantMillis(3_000)),
            MarkDestinationUsedOutcome::Refreshed,
        );
        assert_eq!(
            store.retain(&destination(1)),
            RetainDestinationOutcome::Retained,
        );
        assert_eq!(
            store.mark_used(&destination(1), InstantMillis(4_000)),
            MarkDestinationUsedOutcome::Retained,
        );
        assert_eq!(
            store.get(&destination(1)).unwrap().retention,
            KnownDestinationRetentionState::Retained,
        );
        assert_eq!(
            store.mark_used(&destination(9), InstantMillis(5_000)),
            MarkDestinationUsedOutcome::NotFound,
        );
    }

    #[test]
    fn release_always_records_use_for_a_known_destination() {
        let mut never_used = Store::default();
        remember(&mut never_used, 1, 0xA1, b"", 1_000).unwrap();
        assert_eq!(
            never_used.release(&destination(1), InstantMillis(2_000)),
            ReleaseDestinationOutcome::UseRecorded,
        );

        let mut used = Store::default();
        remember(&mut used, 1, 0xA1, b"", 1_000).unwrap();
        used.mark_used(&destination(1), InstantMillis(2_000));
        assert_eq!(
            used.release(&destination(1), InstantMillis(3_000)),
            ReleaseDestinationOutcome::UseRefreshed,
        );

        let mut retained = Store::default();
        remember(&mut retained, 1, 0xA1, b"", 1_000).unwrap();
        retained.retain(&destination(1));
        assert_eq!(
            retained.release(&destination(1), InstantMillis(4_000)),
            ReleaseDestinationOutcome::Released,
        );
        assert_eq!(
            retained.get(&destination(1)).unwrap().retention,
            KnownDestinationRetentionState::UsedAt(InstantMillis(4_000)),
        );
        assert_eq!(
            retained.release(&destination(9), InstantMillis(5_000)),
            ReleaseDestinationOutcome::NotFound,
        );
    }

    #[test]
    fn identity_retention_counts_new_and_already_retained_destinations() {
        let mut store = Store::default();
        remember(&mut store, 1, 0xA1, b"", 1_000).unwrap();
        remember(&mut store, 2, 0xA1, b"", 1_100).unwrap();
        remember(&mut store, 3, 0xB2, b"", 1_200).unwrap();
        store.retain(&destination(2));

        assert_eq!(
            store.retain_identity(&keys(0xA1).identity_hash()),
            RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 1,
            },
        );
        assert_eq!(
            store.retain_identity(&keys(0xC3).identity_hash()),
            RetainIdentityOutcome {
                newly_retained_destination_count: 0,
                already_retained_destination_count: 0,
            },
        );
    }

    #[test]
    fn cleanup_observes_rns_boundaries_paths_and_retention() {
        let mut store = Store::default();
        remember(&mut store, 1, 0xA1, b"", 1_000).unwrap();
        remember(&mut store, 2, 0xB2, b"", 1_000).unwrap();
        remember(&mut store, 3, 0xC3, b"", 1_000).unwrap();
        store.mark_used(&destination(2), InstantMillis(2_000));
        store.retain(&destination(3));

        assert_eq!(
            store.cull_expired(
                InstantMillis(1_000 + UNUSED_DESTINATION_LINGER_MILLIS),
                |_| false,
            ),
            0,
        );
        assert_eq!(
            store.cull_expired(
                InstantMillis(1_001 + UNUSED_DESTINATION_LINGER_MILLIS),
                |candidate| *candidate == destination(1),
            ),
            0,
        );
        assert_eq!(
            store.cull_expired(
                InstantMillis(1_001 + UNUSED_DESTINATION_LINGER_MILLIS),
                |_| false,
            ),
            1,
        );
        assert_eq!(
            store.cull_expired(
                InstantMillis(2_001 + USED_DESTINATION_LINGER_MILLIS),
                |_| false,
            ),
            1,
        );
        assert!(store.contains(&destination(3)));
    }

    #[test]
    fn soonest_expiry_skips_paths_and_retained_rows() {
        let mut store = Store::default();
        remember(&mut store, 1, 0xA1, b"", 1_000).unwrap();
        remember(&mut store, 2, 0xB2, b"", 2_000).unwrap();
        remember(&mut store, 3, 0xC3, b"", 3_000).unwrap();
        store.retain(&destination(3));

        assert_eq!(
            store.soonest_expiry(|candidate| *candidate == destination(1)),
            Some(InstantMillis(2_001 + UNUSED_DESTINATION_LINGER_MILLIS)),
        );
    }

    #[test]
    fn bounded_storage_can_reclaim_the_oldest_unretained_pathless_row() {
        type SmallTable =
            FixedIndexedKnownDestinationTable<2, { known_destination_index_buckets(2) }>;
        type SmallStore = KnownDestinations<SmallTable, PackedAppDataArena<16, 2>>;
        let mut store = SmallStore::default();
        store
            .remember(destination(1), keys(0xA1), b"one", InstantMillis(1_000))
            .unwrap();
        store
            .remember(destination(2), keys(0xB2), b"two", InstantMillis(2_000))
            .unwrap();

        assert!(store.evict_oldest_unretained_without_path(|_| false));
        assert!(!store.contains(&destination(1)));
        assert!(store.contains(&destination(2)));
        assert_eq!(
            store.remember(destination(3), keys(0xC3), b"three", InstantMillis(3_000)),
            Ok(RememberKnownDestinationOutcome::Remembered),
        );
    }

    #[test]
    fn a_failed_table_insert_releases_its_app_data_slot() {
        type OneTable =
            FixedIndexedKnownDestinationTable<1, { known_destination_index_buckets(1) }>;
        type OneStore = KnownDestinations<OneTable, PackedAppDataArena<16, 2>>;
        let mut store = OneStore::default();
        store
            .remember(destination(1), keys(0xA1), b"first", InstantMillis(0))
            .unwrap();
        assert_eq!(
            store.remember(destination(2), keys(0xB2), b"second", InstantMillis(0)),
            Err(RememberKnownDestinationError::TableFull),
        );
        assert_eq!(
            store.cull_expired(InstantMillis(UNUSED_DESTINATION_LINGER_MILLIS + 1), |_| {
                false
            },),
            1,
        );
        assert_eq!(
            store.remember(destination(3), keys(0xC3), b"third", InstantMillis(1)),
            Ok(RememberKnownDestinationOutcome::Remembered),
        );
    }
}
