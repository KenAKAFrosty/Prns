//! Slot-indexed: slot `i` corresponds to the same-`i` route in the routing-table columns,
//! sharing that table's destination key. The columns live in a caller-chosen heap region
//! (PSRAM on the S3) via `A`; the routing table keeps its keyed index in SRAM.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::routing::announce::stored::{AnnounceRecord, AnnounceRecordColumns, AppDataHandle};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::ColumnsFull;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapAnnounceRecordColumns<
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
    for FixedHeapAnnounceRecordColumns<MAX_TRACKED_DESTINATIONS, A>
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

impl<const MAX_TRACKED_DESTINATIONS: usize, A: Allocator> AnnounceRecordColumns
    for FixedHeapAnnounceRecordColumns<MAX_TRACKED_DESTINATIONS, A>
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

    fn push(&mut self, row: AnnounceRecord) -> Result<usize, ColumnsFull> {
        if self.len >= MAX_TRACKED_DESTINATIONS {
            return Err(ColumnsFull);
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

    type Announces4 = FixedHeapAnnounceRecordColumns<4>;

    #[test]
    fn push_exposes_only_pushed_rows() {
        let mut columns = Announces4::default();
        assert_eq!(columns.capacity(), 4);
        assert!(columns.is_empty());

        assert_eq!(columns.push(row(1)), Ok(0));
        assert_eq!(columns.push(row(2)), Ok(1));

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.public_keys(), &[keys(1), keys(2)]);
        assert_eq!(
            columns.announce_ids(),
            &[
                AnnounceId::from_wire([1; 10]),
                AnnounceId::from_wire([2; 10])
            ]
        );
    }

    #[test]
    fn a_full_table_refuses_the_next_push() {
        let mut columns = Announces4::default();
        for seed in 0..4u8 {
            columns.push(row(seed)).unwrap();
        }
        assert_eq!(columns.len(), 4);
        assert_eq!(columns.push(row(9)), Err(ColumnsFull));
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut columns = Announces4::default();
        columns.push(row(1)).unwrap();
        columns.push(row(2)).unwrap();
        columns.push(row(3)).unwrap();

        columns.swap_remove(0, columns.len() - 1);

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.public_keys(), &[keys(3), keys(2)]);
    }

    #[test]
    fn the_bulk_columns_carry_a_large_table() {
        type Announces2048 = FixedHeapAnnounceRecordColumns<2048>;
        let mut columns = Announces2048::default();
        for seed in 0..2048u32 {
            columns.push(row(seed as u8)).unwrap();
        }
        assert_eq!(columns.len(), 2048);
        assert_eq!(columns.push(row(0)), Err(ColumnsFull));
    }
}
