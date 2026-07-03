use alloc::vec::Vec;

use crate::crypto::ratchets::{LastRotated, SelfRatchetColumns, TrackRatchetsError};
use crate::crypto::X25519SecretKey;
use crate::engine::InstantMillis;
use crate::wire::DestinationHash;

/// RNS 1.3.5 `Destination.RATCHET_COUNT`: how many generated ratchets a
/// destination retains for decryption, newest first.
pub const DEFAULT_RETAINED_RATCHETS: usize = 512;

#[derive(Default)]
pub struct HeapSelfRatchetColumns {
    destinations: Vec<DestinationHash>,
    last_rotated: Vec<LastRotated>,
    secrets: Vec<Vec<X25519SecretKey>>,
}

impl SelfRatchetColumns for HeapSelfRatchetColumns {
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

    fn push(&mut self, destination: DestinationHash) -> Result<usize, TrackRatchetsError> {
        let i = self.destinations.len();
        self.destinations.push(destination);
        self.last_rotated.push(LastRotated::Never);
        self.secrets.push(Vec::new());
        Ok(i)
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
        let mut columns = HeapSelfRatchetColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        assert_eq!(columns.retained_per_destination(), 512);

        assert_eq!(columns.push(dest(1)), Ok(0));
        assert_eq!(columns.push(dest(2)), Ok(1));
        columns.insert_newest_secret(0, secret(0x11));
        columns.insert_newest_secret(0, secret(0x22));
        columns.set_last_rotated(1, InstantMillis(9_000));

        let row = columns.secrets_newest_first(0).unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!(crate::crypto::x25519_public_key(&row[0]).0, public(0x22));
        assert_eq!(crate::crypto::x25519_public_key(&row[1]).0, public(0x11));
        assert_eq!(columns.secrets_newest_first(1).map(<[_]>::len), Some(0));
        assert_eq!(
            columns.last_rotated(),
            &[LastRotated::Never, LastRotated::At(InstantMillis(9_000))]
        );
    }
}
