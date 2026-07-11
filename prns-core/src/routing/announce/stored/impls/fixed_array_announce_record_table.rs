use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::routing::announce::stored::{AnnounceRecord, AnnounceRecordTable, AppDataHandle};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::TablePushError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayAnnounceRecordTable<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    public_keys: [IdentityPublicKeys; MAX_TRACKED_DESTINATIONS],
    dotted_name_hashes: [DottedNameHash; MAX_TRACKED_DESTINATIONS],
    announce_ids: [AnnounceId; MAX_TRACKED_DESTINATIONS],
    ratchets: [Option<RatchetKey>; MAX_TRACKED_DESTINATIONS],
    signatures: [Ed25519Signature; MAX_TRACKED_DESTINATIONS],
    app_data_handles: [Option<AppDataHandle>; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayAnnounceRecordTable<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            public_keys: [IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            }; MAX_TRACKED_DESTINATIONS],
            dotted_name_hashes: [DottedNameHash::new([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            announce_ids: [AnnounceId::from_wire([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            ratchets: [None; MAX_TRACKED_DESTINATIONS],
            signatures: [Ed25519Signature([0u8; 64]); MAX_TRACKED_DESTINATIONS],
            app_data_handles: [None; MAX_TRACKED_DESTINATIONS],
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize> AnnounceRecordTable
    for FixedArrayAnnounceRecordTable<MAX_TRACKED_DESTINATIONS>
{
    fn capacity(&self) -> usize {
        MAX_TRACKED_DESTINATIONS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys[..self.len]
    }
    fn dotted_name_hashes(&self) -> &[DottedNameHash] {
        &self.dotted_name_hashes[..self.len]
    }
    fn announce_ids(&self) -> &[AnnounceId] {
        &self.announce_ids[..self.len]
    }
    fn ratchets(&self) -> &[Option<RatchetKey>] {
        &self.ratchets[..self.len]
    }
    fn signatures(&self) -> &[Ed25519Signature] {
        &self.signatures[..self.len]
    }
    fn app_data_handles(&self) -> &[Option<AppDataHandle>] {
        &self.app_data_handles[..self.len]
    }

    fn set_row(&mut self, i: usize, row: AnnounceRecord) {
        self.public_keys[i] = row.public_keys;
        self.dotted_name_hashes[i] = row.dotted_name_hash;
        self.announce_ids[i] = row.announce_id;
        self.ratchets[i] = row.ratchet;
        self.signatures[i] = row.signature;
        self.app_data_handles[i] = row.maybe_app_data_handle;
    }

    fn push(&mut self, row: AnnounceRecord) -> Result<usize, TablePushError> {
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(TablePushError::TableFull);
        }
        let i = self.len;
        self.set_row(i, row);
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.len - 1);
        self.public_keys[i] = self.public_keys[last];
        self.dotted_name_hashes[i] = self.dotted_name_hashes[last];
        self.announce_ids[i] = self.announce_ids[last];
        self.ratchets[i] = self.ratchets[last];
        self.signatures[i] = self.signatures[last];
        self.app_data_handles[i] = self.app_data_handles[last];
        self.len = last;
    }
}
