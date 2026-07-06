//! Heap-backed announce-record columns: one `Vec` per column, pushed in lockstep so slot
//! indices stay synchronized with the routing table. `capacity()` is `usize::MAX`; `push` is infallible.

use alloc::vec::Vec;

use crate::crypto::Ed25519Signature;
use crate::routing::announce::stored::{AnnounceRecord, AnnounceRecordColumns, AppDataHandle};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::ColumnsFull;

#[derive(Debug, Default)]
pub struct HeapAnnounceRecordColumns {
    public_keys: Vec<IdentityPublicKeys>,
    dotted_name_hashes: Vec<DottedNameHash>,
    announce_ids: Vec<AnnounceId>,
    ratchets: Vec<Option<RatchetKey>>,
    signatures: Vec<Ed25519Signature>,
    app_data_handles: Vec<Option<AppDataHandle>>,
}

impl AnnounceRecordColumns for HeapAnnounceRecordColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.public_keys.len()
    }

    fn public_keys(&self) -> &[IdentityPublicKeys] {
        &self.public_keys
    }
    fn dotted_name_hashes(&self) -> &[DottedNameHash] {
        &self.dotted_name_hashes
    }
    fn announce_ids(&self) -> &[AnnounceId] {
        &self.announce_ids
    }
    fn ratchets(&self) -> &[Option<RatchetKey>] {
        &self.ratchets
    }
    fn signatures(&self) -> &[Ed25519Signature] {
        &self.signatures
    }
    fn app_data_handles(&self) -> &[Option<AppDataHandle>] {
        &self.app_data_handles
    }

    fn set_row(&mut self, i: usize, row: AnnounceRecord) {
        self.public_keys[i] = row.public_keys;
        self.dotted_name_hashes[i] = row.dotted_name_hash;
        self.announce_ids[i] = row.announce_id;
        self.ratchets[i] = row.ratchet;
        self.signatures[i] = row.signature;
        self.app_data_handles[i] = row.maybe_app_data_handle;
    }

    fn push(&mut self, row: AnnounceRecord) -> Result<usize, ColumnsFull> {
        let i = self.public_keys.len();
        self.public_keys.push(row.public_keys);
        self.dotted_name_hashes.push(row.dotted_name_hash);
        self.announce_ids.push(row.announce_id);
        self.ratchets.push(row.ratchet);
        self.signatures.push(row.signature);
        self.app_data_handles.push(row.maybe_app_data_handle);
        Ok(i)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        debug_assert_eq!(last, self.public_keys.len() - 1);
        self.public_keys.swap_remove(i);
        self.dotted_name_hashes.swap_remove(i);
        self.announce_ids.swap_remove(i);
        self.ratchets.swap_remove(i);
        self.signatures.swap_remove(i);
        self.app_data_handles.swap_remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};

    fn entry(byte: u8) -> AnnounceRecord {
        AnnounceRecord {
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([byte; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([byte; 32])),
            },
            dotted_name_hash: DottedNameHash::new([byte; 10]),
            announce_id: AnnounceId::from_wire([byte; 10]),
            signature: Ed25519Signature([byte; 64]),
            ratchet: None,
            maybe_app_data_handle: None,
        }
    }

    #[test]
    fn grows_past_any_fixed_ceiling_and_exposes_only_pushed_rows() {
        let mut columns = HeapAnnounceRecordColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        assert!(columns.is_empty());

        for n in 0..1_000u32 {
            assert_eq!(columns.push(entry(n as u8)), Ok(n as usize));
        }
        assert_eq!(columns.len(), 1_000);
        assert_eq!(columns.signatures().len(), 1_000);

        columns.set_row(0, entry(0xEE));
        assert_eq!(columns.signatures()[0], Ed25519Signature([0xEE; 64]));
        assert_eq!(columns.announce_ids().len(), 1_000);
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut columns = HeapAnnounceRecordColumns::default();
        columns.push(entry(1)).unwrap();
        columns.push(entry(2)).unwrap();
        columns.push(entry(3)).unwrap();

        columns.swap_remove(0, columns.len() - 1);

        assert_eq!(columns.len(), 2);
        assert_eq!(
            columns.signatures(),
            &[Ed25519Signature([3; 64]), Ed25519Signature([2; 64])]
        );
        assert_eq!(
            columns.announce_ids(),
            &[
                AnnounceId::from_wire([3; 10]),
                AnnounceId::from_wire([2; 10])
            ]
        );
    }
}
