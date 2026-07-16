use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::known::{KnownDestinationRecord, KnownDestinationTable};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    KnownDestinationRetentionState,
};
use crate::routing::announce::stored::AppDataHandle;
use crate::storage::TablePushError;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedArrayKnownDestinationTable<const CAPACITY: usize> {
    len: usize,
    destinations: [DestinationHash; CAPACITY],
    public_keys: [IdentityPublicKeys; CAPACITY],
    announced_at: [InstantMillis; CAPACITY],
    retention: [KnownDestinationRetentionState; CAPACITY],
    app_data_handles: [AppDataHandle; CAPACITY],
}

impl<const CAPACITY: usize> Default for FixedArrayKnownDestinationTable<CAPACITY> {
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0; 16]); CAPACITY],
            public_keys: [IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0; 32])),
            }; CAPACITY],
            announced_at: [InstantMillis(0); CAPACITY],
            retention: [KnownDestinationRetentionState::NeverUsed; CAPACITY],
            app_data_handles: [AppDataHandle::new(0); CAPACITY],
        }
    }
}

impl<const CAPACITY: usize> KnownDestinationTable for FixedArrayKnownDestinationTable<CAPACITY> {
    fn capacity(&self) -> usize {
        CAPACITY
    }

    fn len(&self) -> usize {
        self.len
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
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.destinations[index] = self.destinations[last];
            self.public_keys[index] = self.public_keys[last];
            self.announced_at[index] = self.announced_at[last];
            self.retention[index] = self.retention[last];
            self.app_data_handles[index] = self.app_data_handles[last];
        }
        self.len = last;
    }
}
