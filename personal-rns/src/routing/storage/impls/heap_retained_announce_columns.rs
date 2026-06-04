//! Heap-backed, growable retained-announce columns (the typical std/alloc backend).
//!
//! The announce-coherent twin of [`HeapRouteColumns`](super::HeapRouteColumns),
//! one `Vec` per column, pushed in lockstep so slot indices stay synchronized.
//! `capacity()` is `usize::MAX`; `push` is infallible.

use alloc::vec::Vec;

use crate::crypto::Ed25519Signature;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::routing::storage::{
    AppDataHandle, ColumnsFull, RetainedAnnounceColumns, RetainedAnnounceEntry,
};

#[derive(Debug, Default)]
pub struct HeapRetainedAnnounceColumns {
    public_keys: Vec<IdentityPublicKeys>,
    dotted_name_hash: Vec<DottedNameHash>,
    retained_announce_id: Vec<AnnounceId>,
    ratchet: Vec<Option<RatchetKey>>,
    signature: Vec<Ed25519Signature>,
    app_data_handle: Vec<Option<AppDataHandle>>,
}

impl RetainedAnnounceColumns for HeapRetainedAnnounceColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.public_keys.len()
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys
    }
    fn dotted_name_hash(&self) -> &[DottedNameHash] {
        &self.dotted_name_hash
    }
    fn retained_announce_id(&self) -> &[AnnounceId] {
        &self.retained_announce_id
    }
    fn ratchet(&self) -> &[Option<RatchetKey>] {
        &self.ratchet
    }
    fn signature(&self) -> &[Ed25519Signature] {
        &self.signature
    }
    fn app_data_handle(&self) -> &[Option<AppDataHandle>] {
        &self.app_data_handle
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
        let i = self.public_keys.len();
        self.public_keys.push(row.public_keys);
        self.dotted_name_hash.push(row.dotted_name_hash);
        self.retained_announce_id.push(row.retained_announce_id);
        self.ratchet.push(row.maybe_ratchet);
        self.signature.push(row.signature);
        self.app_data_handle.push(row.maybe_app_data_handle);
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};

    fn entry(byte: u8) -> RetainedAnnounceEntry {
        RetainedAnnounceEntry {
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([byte; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([byte; 32])),
            },
            dotted_name_hash: DottedNameHash::new([byte; 10]),
            retained_announce_id: AnnounceId::from_wire([byte; 10]),
            signature: Ed25519Signature([byte; 64]),
            maybe_ratchet: None,
            maybe_app_data_handle: None,
        }
    }

    #[test]
    fn grows_past_any_fixed_ceiling_and_exposes_only_pushed_rows() {
        let mut columns = HeapRetainedAnnounceColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        assert!(columns.is_empty());

        for n in 0..1_000u32 {
            assert_eq!(columns.push(entry(n as u8)), Ok(n as usize));
        }
        assert_eq!(columns.len(), 1_000);
        assert_eq!(columns.signature().len(), 1_000);

        columns.set_row(0, entry(0xEE));
        assert_eq!(columns.signature()[0], Ed25519Signature([0xEE; 64]));
        assert_eq!(columns.retained_announce_id().len(), 1_000);
    }
}
