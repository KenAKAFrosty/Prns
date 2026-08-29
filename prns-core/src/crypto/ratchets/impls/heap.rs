use alloc::vec::Vec;

use crate::crypto::ratchets::{LastRotated, SelfRatchetTable, TrackRatchetsError};
use crate::crypto::X25519SecretKey;
use crate::engine::InstantMillis;
use crate::wire::DestinationHash;

/// RNS 1.4.2 `Destination.RATCHET_COUNT`: how many generated ratchets a destination retains for decryption, newest first.
pub const DEFAULT_RETAINED_RATCHETS: usize = 512;

#[derive(Default)]
pub struct HeapSelfRatchetTable {
    destinations: Vec<DestinationHash>,
    last_rotated: Vec<LastRotated>,
    secrets: Vec<Vec<X25519SecretKey>>,
}

impl SelfRatchetTable for HeapSelfRatchetTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn retained_per_destination(&self) -> usize {
        DEFAULT_RETAINED_RATCHETS
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
        if row.len() >= DEFAULT_RETAINED_RATCHETS {
            row.pop();
        }
        row.insert(0, secret);
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
        self.destinations.push(destination);
        self.last_rotated.push(LastRotated::Never);
        self.secrets.push(Vec::new());
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

    fn public(byte: u8) -> [u8; 32] {
        crate::crypto::x25519_public_key(&secret(byte)).0
    }

    #[test]
    fn grows_without_a_destination_cap_and_keeps_newest_first() {
        let mut table = HeapSelfRatchetTable::default();
        assert_eq!(table.capacity(), usize::MAX);
        assert_eq!(table.retained_per_destination(), 512);

        assert_eq!(table.push(dest(1)), Ok(()));
        assert_eq!(table.push(dest(2)), Ok(()));
        table.insert_newest_secret(0, secret(0x11));
        table.insert_newest_secret(0, secret(0x22));
        table.set_last_rotated(1, InstantMillis(9_000));

        let row = table.secrets_newest_first(0).unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!(crate::crypto::x25519_public_key(&row[0]).0, public(0x22));
        assert_eq!(crate::crypto::x25519_public_key(&row[1]).0, public(0x11));
        assert_eq!(table.secrets_newest_first(1).map(<[_]>::len), Some(0));
        assert_eq!(
            table.last_rotated(),
            &[LastRotated::Never, LastRotated::At(InstantMillis(9_000))]
        );
    }
}
