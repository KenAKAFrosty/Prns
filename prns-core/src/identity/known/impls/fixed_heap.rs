use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::known::impls::fixed::known_destination_index_buckets;
use crate::identity::known::{KnownDestinationRecord, KnownDestinationTable};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    KnownDestinationRetentionState,
};
use crate::lemire_index::LemireIndex;
use crate::routing::announce::stored::AppDataHandle;
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, allocator: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, allocator);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapKnownDestinationTable<
    const CAPACITY: usize,
    const INDEX_BUCKETS: usize,
    A: Allocator = Global,
> {
    len: usize,
    destinations: Box<[DestinationHash], A>,
    public_keys: Box<[IdentityPublicKeys], A>,
    announced_at: Box<[InstantMillis], A>,
    retention: Box<[KnownDestinationRetentionState], A>,
    app_data_handles: Box<[AppDataHandle], A>,
    index: LemireIndex<INDEX_BUCKETS>,
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize, A: Allocator + Default> Default
    for FixedHeapKnownDestinationTable<CAPACITY, INDEX_BUCKETS, A>
{
    fn default() -> Self {
        const {
            assert!(
                INDEX_BUCKETS >= known_destination_index_buckets(CAPACITY),
                "INDEX_BUCKETS must preserve two-thirds-load headroom over CAPACITY",
            );
            assert!(
                CAPACITY < u16::MAX as usize,
                "FixedHeapKnownDestinationTable indexes slots as u16",
            );
        }
        Self {
            len: 0,
            destinations: filled(DestinationHash::new([0; 16]), CAPACITY, A::default()),
            public_keys: filled(
                IdentityPublicKeys {
                    encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0; 32])),
                    signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0; 32])),
                },
                CAPACITY,
                A::default(),
            ),
            announced_at: filled(InstantMillis(0), CAPACITY, A::default()),
            retention: filled(
                KnownDestinationRetentionState::NeverUsed,
                CAPACITY,
                A::default(),
            ),
            app_data_handles: filled(AppDataHandle::new(0), CAPACITY, A::default()),
            index: LemireIndex::default(),
        }
    }
}

impl<const CAPACITY: usize, const INDEX_BUCKETS: usize, A: Allocator> KnownDestinationTable
    for FixedHeapKnownDestinationTable<CAPACITY, INDEX_BUCKETS, A>
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

    fn retention(&self) -> &[KnownDestinationRetentionState] {
        &self.retention[..self.len]
    }

    fn app_data_handles(&self) -> &[AppDataHandle] {
        &self.app_data_handles[..self.len]
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
