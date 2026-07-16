use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::destination_identity::{DestinationIdentityRecord, DestinationIdentityTable};
use crate::identity::{
    DestinationIdentityRetentionState, IdentityEncryptionPublicKey, IdentityPublicKeys,
    IdentitySigningPublicKey,
};
use crate::lemire_index::{buckets_for_two_thirds_load, LemireIndex};
use crate::routing::announce::stored::AppDataHandle;
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

pub const fn destination_identity_index_buckets(entries: usize) -> usize {
    buckets_for_two_thirds_load(entries)
}

#[derive(Debug)]
pub struct FixedIndexedDestinationIdentityTable<const CAPACITY: usize, const INDEX_BUCKETS: usize> {
    len: usize,
    destinations: [DestinationHash; CAPACITY],
    public_keys: [IdentityPublicKeys; CAPACITY],
    announced_at: [InstantMillis; CAPACITY],
    retention: [DestinationIdentityRetentionState; CAPACITY],
    app_data_handles: [AppDataHandle; CAPACITY],
    index: LemireIndex<INDEX_BUCKETS>,
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize> Default
    for FixedIndexedDestinationIdentityTable<CAPACITY, INDEX_BUCKETS>
{
    fn default() -> Self {
        const {
            assert!(
                INDEX_BUCKETS >= destination_identity_index_buckets(CAPACITY),
                "INDEX_BUCKETS must preserve two-thirds-load headroom over CAPACITY",
            );
            assert!(
                CAPACITY < u16::MAX as usize,
                "FixedIndexedDestinationIdentityTable indexes slots as u16",
            );
        }
        Self {
            len: 0,
            destinations: [DestinationHash::new([0; 16]); CAPACITY],
            public_keys: [IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0; 32])),
            }; CAPACITY],
            announced_at: [InstantMillis(0); CAPACITY],
            retention: [DestinationIdentityRetentionState::NeverUsed; CAPACITY],
            app_data_handles: [AppDataHandle::new(0); CAPACITY],
            index: LemireIndex::default(),
        }
    }
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize> DestinationIdentityTable
    for FixedIndexedDestinationIdentityTable<CAPACITY, INDEX_BUCKETS>
{
    fn capacity(&self) -> usize {
        CAPACITY
    }

    fn len(&self) -> usize {
        self.len
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys[..self.len]
    }

    fn announced_at(&self) -> &[InstantMillis] {
        &self.announced_at[..self.len]
    }

    fn retention(&self) -> &[DestinationIdentityRetentionState] {
        &self.retention[..self.len]
    }

    fn app_data_handles(&self) -> &[AppDataHandle] {
        &self.app_data_handles[..self.len]
    }

    fn set_row(&mut self, index: usize, record: DestinationIdentityRecord) {
        self.public_keys[index] = record.public_keys;
        self.announced_at[index] = record.announced_at;
        self.retention[index] = record.retention;
        self.app_data_handles[index] = record.app_data_handle;
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        record: DestinationIdentityRecord,
    ) -> Result<usize, TablePushError> {
        if self.len >= CAPACITY {
            return Err(TablePushError::TableFull);
        }
        let index = self.len;
        self.destinations[index] = destination;
        self.set_row(index, record);
        self.len += 1;
        self.index.insert(index, &self.destinations);
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        self.index.remove_slot(index, &self.destinations);
        if index != last {
            let moved = self.destinations[last];
            self.index.repoint(&moved, index, &self.destinations);
            self.destinations[index] = self.destinations[last];
            self.public_keys[index] = self.public_keys[last];
            self.announced_at[index] = self.announced_at[last];
            self.retention[index] = self.retention[last];
            self.app_data_handles[index] = self.app_data_handles[last];
        }
        self.len = last;
    }
}
