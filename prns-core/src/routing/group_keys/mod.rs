//! The shared symmetric keys held by GROUP destinations (RNS 1.3.5 `Destination` type
//! `0x01`): one `Token.generate_key()` secret, encrypting and decrypting with it directly.

mod impls;

pub use impls::*;

use crate::storage::ColumnsFull;
use crate::wire::DestinationHash;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// RNS `Token.generate_key()` defaults to an AES-256 key (32-byte signing half
/// ‖ 32-byte encryption half); the AES-128 form is 32 bytes. Both are valid.
pub const GROUP_KEY_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKeyError {
    InvalidLength,
}

/// A GROUP destination's shared symmetric key, sized to the AES-256 ceiling and
/// carrying its true length so a 32-byte AES-128 key round-trips too.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct GroupKey {
    material: [u8; GROUP_KEY_MAX_LEN],
    len: usize,
}

impl GroupKey {
    pub fn from_slice(key: &[u8]) -> Result<Self, GroupKeyError> {
        if key.len() != 32 && key.len() != GROUP_KEY_MAX_LEN {
            return Err(GroupKeyError::InvalidLength);
        }
        let mut material = [0u8; GROUP_KEY_MAX_LEN];
        material[..key.len()].copy_from_slice(key);
        Ok(Self {
            material,
            len: key.len(),
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.material[..self.len]
    }
}

impl Default for GroupKey {
    fn default() -> Self {
        Self {
            material: [0u8; GROUP_KEY_MAX_LEN],
            len: 0,
        }
    }
}

impl core::fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GroupKey").field("len", &self.len).finish()
    }
}

pub trait GroupKeyColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn keys(&self) -> &[GroupKey];

    fn upsert(&mut self, destination: DestinationHash, key: GroupKey) -> Result<(), ColumnsFull>;
}

#[derive(Debug, Default)]
pub struct GroupKeys<C: GroupKeyColumns> {
    columns: C,
}

impl<C: GroupKeyColumns> GroupKeys<C> {
    pub fn insert(
        &mut self,
        destination: DestinationHash,
        key: GroupKey,
    ) -> Result<(), ColumnsFull> {
        self.columns.upsert(destination, key)
    }

    pub fn has_room(&self) -> bool {
        self.columns.len() < self.columns.capacity()
    }

    pub fn key_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        let slot = self
            .columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        self.columns.keys().get(slot).map(GroupKey::as_slice)
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestGroupKeys = GroupKeys<FixedGroupKeyColumns<4>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn a_stored_key_round_trips_by_destination() {
        let mut keys = TestGroupKeys::default();
        let key = GroupKey::from_slice(&[0xAB; 64]).unwrap();
        keys.insert(dest(1), key).unwrap();

        assert_eq!(keys.key_for(&dest(1)), Some([0xAB; 64].as_slice()));
        assert_eq!(keys.key_for(&dest(2)), None);
    }

    #[test]
    fn both_aes_128_and_aes_256_key_lengths_store_and_retrieve() {
        let mut keys = TestGroupKeys::default();
        keys.insert(dest(1), GroupKey::from_slice(&[0x11; 32]).unwrap())
            .unwrap();
        keys.insert(dest(2), GroupKey::from_slice(&[0x22; 64]).unwrap())
            .unwrap();

        assert_eq!(keys.key_for(&dest(1)), Some([0x11; 32].as_slice()));
        assert_eq!(keys.key_for(&dest(2)), Some([0x22; 64].as_slice()));
    }

    #[test]
    fn a_key_length_that_is_neither_aes_128_nor_aes_256_is_rejected() {
        assert!(matches!(
            GroupKey::from_slice(&[0u8; 48]),
            Err(GroupKeyError::InvalidLength)
        ));
        assert!(matches!(
            GroupKey::from_slice(&[]),
            Err(GroupKeyError::InvalidLength)
        ));
        assert!(GroupKey::from_slice(&[0u8; 32]).is_ok());
        assert!(GroupKey::from_slice(&[0u8; 64]).is_ok());
    }

    #[test]
    fn re_registering_a_destination_overwrites_its_key_in_place() {
        let mut keys = TestGroupKeys::default();
        keys.insert(dest(1), GroupKey::from_slice(&[0x11; 64]).unwrap())
            .unwrap();
        keys.insert(dest(1), GroupKey::from_slice(&[0x99; 64]).unwrap())
            .unwrap();

        assert_eq!(keys.len(), 1);
        assert_eq!(keys.key_for(&dest(1)), Some([0x99; 64].as_slice()));
    }

    #[test]
    fn a_full_fixed_store_reports_itself_but_still_overwrites_known_destinations() {
        let mut keys = GroupKeys::<FixedGroupKeyColumns<2>>::default();
        keys.insert(dest(1), GroupKey::default()).unwrap();
        assert!(keys.has_room());
        keys.insert(dest(2), GroupKey::default()).unwrap();
        assert!(!keys.has_room());

        assert_eq!(keys.insert(dest(3), GroupKey::default()), Err(ColumnsFull));
        assert_eq!(
            keys.insert(dest(1), GroupKey::from_slice(&[0x77; 32]).unwrap()),
            Ok(())
        );
        assert_eq!(keys.key_for(&dest(1)), Some([0x77; 32].as_slice()));
    }

    #[test]
    fn heap_columns_track_past_any_fixed_ceiling() {
        let mut keys = GroupKeys::<HeapGroupKeyColumns>::default();
        for byte in 0..16u8 {
            keys.insert(dest(byte), GroupKey::from_slice(&[byte; 32]).unwrap())
                .unwrap();
        }
        assert_eq!(keys.len(), 16);
        assert_eq!(keys.key_for(&dest(7)), Some([7u8; 32].as_slice()));
    }
}
