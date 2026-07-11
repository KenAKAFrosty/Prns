use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::routing::announce::stored::{AnnounceRecord, AnnounceRecordTable, AppDataHandle};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::TablePushError;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapAnnounceRecordTable<
    const MAX_TRACKED_DESTINATIONS: usize,
    A: Allocator = Global,
> {
    len: usize,
    public_keys: Box<[IdentityPublicKeys], A>,
    dotted_name_hashes: Box<[DottedNameHash], A>,
    announce_ids: Box<[AnnounceId], A>,
    ratchets: Box<[Option<RatchetKey>], A>,
    signatures: Box<[Ed25519Signature], A>,
    app_data_handles: Box<[Option<AppDataHandle>], A>,
}

impl<const MAX_TRACKED_DESTINATIONS: usize, A: Allocator + Default> Default
    for FixedHeapAnnounceRecordTable<MAX_TRACKED_DESTINATIONS, A>
{
    fn default() -> Self {
        let n = MAX_TRACKED_DESTINATIONS;
        Self {
            len: 0,
            public_keys: filled(
                IdentityPublicKeys {
                    encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                    signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
                },
                n,
                A::default(),
            ),
            dotted_name_hashes: filled(DottedNameHash::new([0u8; 10]), n, A::default()),
            announce_ids: filled(AnnounceId::from_wire([0u8; 10]), n, A::default()),
            ratchets: filled(None, n, A::default()),
            signatures: filled(Ed25519Signature([0u8; 64]), n, A::default()),
            app_data_handles: filled(None, n, A::default()),
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize, A: Allocator> AnnounceRecordTable
    for FixedHeapAnnounceRecordTable<MAX_TRACKED_DESTINATIONS, A>
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

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(seed: u8) -> IdentityPublicKeys {
        IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
        }
    }

    fn row(seed: u8) -> AnnounceRecord {
        AnnounceRecord {
            public_keys: keys(seed),
            dotted_name_hash: DottedNameHash::new([seed; 10]),
            announce_id: AnnounceId::from_wire([seed; 10]),
            signature: Ed25519Signature([seed; 64]),
            ratchet: None,
            maybe_app_data_handle: None,
        }
    }

    type Announces4 = FixedHeapAnnounceRecordTable<4>;

    #[test]
    fn push_exposes_only_pushed_rows() {
        let mut table = Announces4::default();
        assert_eq!(table.capacity(), 4);
        assert!(table.is_empty());

        assert_eq!(table.push(row(1)), Ok(0));
        assert_eq!(table.push(row(2)), Ok(1));

        assert_eq!(table.len(), 2);
        assert_eq!(table.public_keys(), &[keys(1), keys(2)]);
        assert_eq!(
            table.announce_ids(),
            &[
                AnnounceId::from_wire([1; 10]),
                AnnounceId::from_wire([2; 10])
            ]
        );
    }

    #[test]
    fn a_full_table_refuses_the_next_push() {
        let mut table = Announces4::default();
        for seed in 0..4u8 {
            table.push(row(seed)).unwrap();
        }
        assert_eq!(table.len(), 4);
        assert_eq!(table.push(row(9)), Err(TablePushError::TableFull));
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut table = Announces4::default();
        table.push(row(1)).unwrap();
        table.push(row(2)).unwrap();
        table.push(row(3)).unwrap();

        table.swap_remove(0, table.len() - 1);

        assert_eq!(table.len(), 2);
        assert_eq!(table.public_keys(), &[keys(3), keys(2)]);
    }

    #[test]
    fn the_bulk_columns_carry_a_large_table() {
        type Announces2048 = FixedHeapAnnounceRecordTable<2048>;
        let mut table = Announces2048::default();
        for seed in 0..2048u32 {
            table.push(row(seed as u8)).unwrap();
        }
        assert_eq!(table.len(), 2048);
        assert_eq!(table.push(row(0)), Err(TablePushError::TableFull));
    }
}
