use alloc::vec::Vec;

use crate::identity::known::{KnownDestinationRecord, KnownDestinationTable};
use crate::identity::IdentityPublicKeys;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::announce::stored::AppDataHandle;
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapKnownDestinationTable {
    destinations: Vec<DestinationHash>,
    public_keys: Vec<IdentityPublicKeys>,
    announced_at: Vec<InstantMillis>,
    retention: Vec<crate::identity::KnownDestinationRetentionState>,
    app_data_handles: Vec<AppDataHandle>,
    index: HeapLemireIndex,
}

impl KnownDestinationTable for HeapKnownDestinationTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys
    }

    fn announced_at(&self) -> &[InstantMillis] {
        &self.announced_at
    }

    fn retention(&self) -> &[crate::identity::KnownDestinationRetentionState] {
        &self.retention
    }

    fn app_data_handles(&self) -> &[AppDataHandle] {
        &self.app_data_handles
    }

    fn set_row(&mut self, index: usize, record: KnownDestinationRecord) {
        self.public_keys[index] = record.public_keys;
        self.announced_at[index] = record.announced_at;
        self.retention[index] = record.retention;
        self.app_data_handles[index] = record.app_data_handle;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        record: KnownDestinationRecord,
    ) -> Result<usize, TablePushError> {
        let index = self.destinations.len();
        self.destinations.push(destination);
        self.public_keys.push(record.public_keys);
        self.announced_at.push(record.announced_at);
        self.retention.push(record.retention);
        self.app_data_handles.push(record.app_data_handle);
        self.index.insert(index, &self.destinations);
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.destinations.len() - 1;
        self.index.remove_slot(index, &self.destinations);
        if index != last {
            self.index.repoint_slot(last, index, &self.destinations);
        }
        self.destinations.swap_remove(index);
        self.public_keys.swap_remove(index);
        self.announced_at.swap_remove(index);
        self.retention.swap_remove(index);
        self.app_data_handles.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentitySigningPublicKey, KnownDestinationRetentionState,
    };

    fn destination(n: u32) -> DestinationHash {
        let key = u64::from(n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&key.to_be_bytes());
        bytes[8..12].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(bytes)
    }

    fn record(byte: u8) -> KnownDestinationRecord {
        KnownDestinationRecord {
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([byte; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([byte; 32])),
            },
            announced_at: InstantMillis(u64::from(byte)),
            retention: KnownDestinationRetentionState::NeverUsed,
            app_data_handle: AppDataHandle::new(usize::from(byte)),
        }
    }

    #[test]
    fn grows_without_a_ceiling_and_indexes_every_destination() {
        let mut table = HeapKnownDestinationTable::default();
        for n in 0..1_000u32 {
            assert_eq!(table.push(destination(n), record(n as u8)), Ok(n as usize));
        }
        assert_eq!(table.capacity(), usize::MAX);
        for n in 0..1_000u32 {
            assert_eq!(table.index_of(&destination(n)), Some(n as usize));
        }
        assert_eq!(table.index_of(&destination(2_000)), None);
    }

    #[test]
    fn the_index_tracks_rows_moved_by_removal() {
        let mut table = HeapKnownDestinationTable::default();
        table.push(destination(1), record(1)).unwrap();
        table.push(destination(2), record(2)).unwrap();
        table.push(destination(3), record(3)).unwrap();

        table.swap_remove(0);

        assert_eq!(table.index_of(&destination(1)), None);
        assert_eq!(table.index_of(&destination(3)), Some(0));
        assert_eq!(table.index_of(&destination(2)), Some(1));
    }
}
