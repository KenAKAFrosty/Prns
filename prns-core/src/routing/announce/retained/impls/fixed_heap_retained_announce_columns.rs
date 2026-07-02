//! Slot-indexed: the slot at index `i` here corresponds to the same-`i` route
//! in the routing-table columns, sharing the destination key from that table.
//! The fixed-capacity, heap-backed twin of [`FixedArrayRetainedAnnounceColumns`]:
//! the columns live in a caller-chosen heap region (PSRAM on the S3) via the
//! allocator `A`, while the routing table keeps its keyed index in SRAM.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::routing::announce::retained::{
    AppDataHandle, RetainedAnnounceColumns, RetainedAnnounceEntry,
};
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::storage::ColumnsFull;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapRetainedAnnounceColumns<
    const MAX_TRACKED_DESTINATIONS: usize,
    A: Allocator = Global,
> {
    len: usize,
    public_keys: Box<[IdentityPublicKeys], A>,
    dotted_name_hash: Box<[DottedNameHash], A>,
    retained_announce_id: Box<[AnnounceId], A>,
    ratchet: Box<[Option<RatchetKey>], A>,
    signature: Box<[Ed25519Signature], A>,
    app_data_handle: Box<[Option<AppDataHandle>], A>,
}

impl<const MAX_TRACKED_DESTINATIONS: usize, A: Allocator + Default> Default
    for FixedHeapRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS, A>
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
            dotted_name_hash: filled(DottedNameHash::new([0u8; 10]), n, A::default()),
            retained_announce_id: filled(AnnounceId::from_wire([0u8; 10]), n, A::default()),
            ratchet: filled(None, n, A::default()),
            signature: filled(Ed25519Signature([0u8; 64]), n, A::default()),
            app_data_handle: filled(None, n, A::default()),
        }
    }
}

impl<const MAX_TRACKED_DESTINATIONS: usize, A: Allocator> RetainedAnnounceColumns
    for FixedHeapRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS, A>
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

    fn swap_remove(&mut self, i: usize) {
        let last = self.len - 1;
        self.public_keys[i] = self.public_keys[last];
        self.dotted_name_hash[i] = self.dotted_name_hash[last];
        self.retained_announce_id[i] = self.retained_announce_id[last];
        self.ratchet[i] = self.ratchet[last];
        self.signature[i] = self.signature[last];
        self.app_data_handle[i] = self.app_data_handle[last];
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

    fn row(seed: u8) -> RetainedAnnounceEntry {
        RetainedAnnounceEntry {
            public_keys: keys(seed),
            dotted_name_hash: DottedNameHash::new([seed; 10]),
            retained_announce_id: AnnounceId::from_wire([seed; 10]),
            signature: Ed25519Signature([seed; 64]),
            maybe_ratchet: None,
            maybe_app_data_handle: None,
        }
    }

    type Announces4 = FixedHeapRetainedAnnounceColumns<4>;

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
            columns.retained_announce_id(),
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

        columns.swap_remove(0);

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.public_keys(), &[keys(3), keys(2)]);
    }

    #[test]
    fn the_bulk_columns_carry_a_large_table() {
        type Announces2048 = FixedHeapRetainedAnnounceColumns<2048>;
        let mut columns = Announces2048::default();
        for seed in 0..2048u32 {
            columns.push(row(seed as u8)).unwrap();
        }
        assert_eq!(columns.len(), 2048);
        assert_eq!(columns.push(row(0)), Err(ColumnsFull));
    }
}
