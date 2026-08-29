use crate::crypto::ratchets::{LastRotated, SelfRatchetTable, TrackRatchetsError};
use crate::crypto::X25519SecretKey;
use crate::engine::InstantMillis;
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;

pub struct FixedSelfRatchetTable<
    const MAX_RATCHETED_DESTINATIONS: usize,
    const RETAINED_RATCHETS_PER_DESTINATION: usize,
> {
    destinations: HeaplessVec<DestinationHash, MAX_RATCHETED_DESTINATIONS>,
    last_rotated: HeaplessVec<LastRotated, MAX_RATCHETED_DESTINATIONS>,
    secrets: HeaplessVec<
        HeaplessVec<X25519SecretKey, RETAINED_RATCHETS_PER_DESTINATION>,
        MAX_RATCHETED_DESTINATIONS,
    >,
}

impl<const MAX_RATCHETED_DESTINATIONS: usize, const RETAINED_RATCHETS_PER_DESTINATION: usize>
    Default
    for FixedSelfRatchetTable<MAX_RATCHETED_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>
{
    fn default() -> Self {
        Self {
            destinations: HeaplessVec::new(),
            last_rotated: HeaplessVec::new(),
            secrets: HeaplessVec::new(),
        }
    }
}

impl<const MAX_RATCHETED_DESTINATIONS: usize, const RETAINED_RATCHETS_PER_DESTINATION: usize>
    SelfRatchetTable
    for FixedSelfRatchetTable<MAX_RATCHETED_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>
{
    fn capacity(&self) -> usize {
        MAX_RATCHETED_DESTINATIONS
    }
    fn retained_per_destination(&self) -> usize {
        RETAINED_RATCHETS_PER_DESTINATION
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn last_rotated(&self) -> &[LastRotated] {
        &self.last_rotated
    }
    fn secrets_newest_first(&self, index: usize) -> Option<&[X25519SecretKey]> {
        self.secrets.get(index).map(|row| row.as_slice())
    }

    fn set_last_rotated(&mut self, index: usize, at: InstantMillis) {
        if let Some(slot) = self.last_rotated.get_mut(index) {
            *slot = LastRotated::At(at);
        }
    }

    fn insert_newest_secret(&mut self, index: usize, secret: X25519SecretKey) {
        let Some(row) = self.secrets.get_mut(index) else {
            return;
        };
        if row.is_full() {
            row.pop();
        }
        let _ = row.insert(0, secret);
    }

    fn clear_secrets(&mut self, index: usize) {
        if let Some(row) = self.secrets.get_mut(index) {
            row.clear();
        }
        if let Some(slot) = self.last_rotated.get_mut(index) {
            *slot = LastRotated::Never;
        }
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.destinations.len() {
            return;
        }
        self.destinations.swap_remove(index);
        self.last_rotated.swap_remove(index);
        self.secrets.swap_remove(index);
    }

    fn push(&mut self, destination: DestinationHash) -> Result<(), TrackRatchetsError> {
        self.destinations
            .push(destination)
            .map_err(|_| TrackRatchetsError::TableFull)?;
        let _ = self.last_rotated.push(LastRotated::Never);
        let _ = self.secrets.push(HeaplessVec::new());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn secret(byte: u8) -> X25519SecretKey {
        X25519SecretKey::new([byte; 32])
    }

    fn secret_bytes(row: &[X25519SecretKey]) -> std::vec::Vec<[u8; 32]> {
        row.iter()
            .map(|s| crate::crypto::x25519_public_key(s).0)
            .collect()
    }

    fn public(byte: u8) -> [u8; 32] {
        crate::crypto::x25519_public_key(&secret(byte)).0
    }

    #[test]
    fn exposes_only_pushed_rows_and_reports_a_full_table() {
        let mut table = FixedSelfRatchetTable::<2, 3>::default();
        assert_eq!(table.capacity(), 2);
        assert_eq!(table.retained_per_destination(), 3);
        assert!(table.is_empty());

        assert_eq!(table.push(dest(1)), Ok(()));
        assert_eq!(table.push(dest(2)), Ok(()));
        assert_eq!(table.push(dest(3)), Err(TrackRatchetsError::TableFull));

        assert_eq!(table.len(), 2);
        assert_eq!(table.destinations(), &[dest(1), dest(2)]);
        assert_eq!(
            table.last_rotated(),
            &[LastRotated::Never, LastRotated::Never]
        );
        assert_eq!(table.secrets_newest_first(0).map(<[_]>::len), Some(0));
        assert!(table.secrets_newest_first(2).is_none());
    }

    #[test]
    fn inserts_keep_newest_first_and_trim_the_oldest_at_the_retained_cap() {
        let mut table = FixedSelfRatchetTable::<1, 2>::default();
        table.push(dest(1)).unwrap();

        table.insert_newest_secret(0, secret(0x11));
        table.insert_newest_secret(0, secret(0x22));
        assert_eq!(
            secret_bytes(table.secrets_newest_first(0).unwrap()),
            &[public(0x22), public(0x11)],
        );

        table.insert_newest_secret(0, secret(0x33));
        assert_eq!(
            secret_bytes(table.secrets_newest_first(0).unwrap()),
            &[public(0x33), public(0x22)],
        );
    }

    #[test]
    fn out_of_range_writes_are_ignored() {
        let mut table = FixedSelfRatchetTable::<1, 2>::default();
        table.push(dest(1)).unwrap();

        table.insert_newest_secret(5, secret(0x11));
        table.set_last_rotated(5, InstantMillis(1_000));

        assert_eq!(table.secrets_newest_first(0).map(<[_]>::len), Some(0));
        assert_eq!(table.last_rotated(), &[LastRotated::Never]);
    }

    #[test]
    fn set_last_rotated_anchors_the_row() {
        let mut table = FixedSelfRatchetTable::<1, 2>::default();
        table.push(dest(1)).unwrap();
        table.set_last_rotated(0, InstantMillis(7_000));
        assert_eq!(
            table.last_rotated(),
            &[LastRotated::At(InstantMillis(7_000))]
        );
    }
}
