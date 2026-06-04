//! Slot-indexed: the slot at index `i` here corresponds to the
//! same-`i` route in the routing-table columns, sharing the destination
//! key from that table.

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::routing::storage::{
    AppDataHandle, ColumnsFull, RetainedAnnounceColumns, RetainedAnnounceEntry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedArrayRetainedAnnounceColumns<const MAX_TRACKED_DESTINATIONS: usize> {
    len: usize,
    public_keys: [IdentityPublicKeys; MAX_TRACKED_DESTINATIONS],
    dotted_name_hash: [DottedNameHash; MAX_TRACKED_DESTINATIONS],
    retained_announce_id: [AnnounceId; MAX_TRACKED_DESTINATIONS],
    ratchet: [Option<RatchetKey>; MAX_TRACKED_DESTINATIONS],
    signature: [Ed25519Signature; MAX_TRACKED_DESTINATIONS],
    app_data_handle: [Option<AppDataHandle>; MAX_TRACKED_DESTINATIONS],
}

impl<const MAX_TRACKED_DESTINATIONS: usize> Default
    for FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            len: 0,
            public_keys: [IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            }; MAX_TRACKED_DESTINATIONS],
            dotted_name_hash: [DottedNameHash::new([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            retained_announce_id: [AnnounceId::from_wire([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            ratchet: [None; MAX_TRACKED_DESTINATIONS],
            signature: [Ed25519Signature([0u8; 64]); MAX_TRACKED_DESTINATIONS],
            app_data_handle: [None; MAX_TRACKED_DESTINATIONS],
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize> RetainedAnnounceColumns
    for FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>
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
    fn dotted_name_hash(&self) -> &[DottedNameHash] {
        &self.dotted_name_hash[..self.len]
    }
    fn retained_announce_id(&self) -> &[AnnounceId] {
        &self.retained_announce_id[..self.len]
    }
    fn ratchet(&self) -> &[Option<RatchetKey>] {
        &self.ratchet[..self.len]
    }
    fn signature(&self) -> &[Ed25519Signature] {
        &self.signature[..self.len]
    }
    fn app_data_handle(&self) -> &[Option<AppDataHandle>] {
        &self.app_data_handle[..self.len]
    }

    fn set_row(&mut self, i: usize, row: RetainedAnnounceEntry) {
        self.public_keys[i] = row.public_keys;
        self.dotted_name_hash[i] = row.dotted_name_hash;
        self.retained_announce_id[i] = row.retained_announce_id;
        self.ratchet[i] = row.maybe_ratchet;
        self.signature[i] = row.signature;
        self.app_data_handle[i] = row.maybe_app_data_handle;
    }

    fn push(&mut self, row: RetainedAnnounceEntry) -> Result<usize, ColumnsFull> {
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(ColumnsFull);
        }
        let i = self.len;
        self.set_row(i, row);
        self.len += 1;
        Ok(i)
    }
}
