use crate::crypto::TokenKey;
use crate::storage::TablePushError;
use crate::wire::DestinationHash;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKeyError {
    InvalidLength,
}

/// A GROUP destination's shared symmetric key. RNS 1.4.2 `Token.generate_key()` mints the AES-256 form by default (32-byte signing half ‖ 32-byte encryption half); the AES-128 form is 32 bytes. The length law lives in the variants, so a stored key views as a [`TokenKey`] without a fallible re-parse per packet.
#[derive(Zeroize, ZeroizeOnDrop)]
pub enum GroupKey {
    Aes128([u8; 32]),
    Aes256([u8; 64]),
}

impl GroupKey {
    pub fn from_slice(key: &[u8]) -> Result<Self, GroupKeyError> {
        if let Ok(key) = <[u8; 32]>::try_from(key) {
            return Ok(Self::Aes128(key));
        }
        if let Ok(key) = <[u8; 64]>::try_from(key) {
            return Ok(Self::Aes256(key));
        }
        Err(GroupKeyError::InvalidLength)
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Aes128(key) => key,
            Self::Aes256(key) => key,
        }
    }

    pub fn as_token_key(&self) -> TokenKey<'_> {
        match self {
            Self::Aes128(key) => TokenKey::from_aes128(key),
            Self::Aes256(key) => TokenKey::from_aes256(key),
        }
    }
}

impl Default for GroupKey {
    fn default() -> Self {
        Self::Aes128([0u8; 32])
    }
}

impl core::fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Aes128(_) => f.write_str("GroupKey::Aes128"),
            Self::Aes256(_) => f.write_str("GroupKey::Aes256"),
        }
    }
}

pub trait GroupKeyTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn keys(&self) -> &[GroupKey];

    fn swap_remove(&mut self, index: usize);
    fn upsert(&mut self, destination: DestinationHash, key: GroupKey)
        -> Result<(), TablePushError>;
}

#[derive(Debug, Default)]
pub struct GroupKeys<C: GroupKeyTable> {
    table: C,
}

impl<C: GroupKeyTable> GroupKeys<C> {
    pub fn insert(
        &mut self,
        destination: DestinationHash,
        key: GroupKey,
    ) -> Result<(), TablePushError> {
        self.table.upsert(destination, key)
    }

    pub fn has_room(&self) -> bool {
        self.table.len() < self.table.capacity()
    }

    pub fn key_for(&self, destination: &DestinationHash) -> Option<&GroupKey> {
        let slot = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        self.table.keys().get(slot)
    }

    pub(crate) fn remove(&mut self, destination: &DestinationHash) {
        let Some(slot) = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        else {
            return;
        };
        self.table.swap_remove(slot);
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    type TestGroupKeys = GroupKeys<FixedGroupKeyTable<4>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn a_stored_key_round_trips_by_destination() {
        let mut keys = TestGroupKeys::default();
        let key = GroupKey::from_slice(&[0xAB; 64]).unwrap();
        keys.insert(dest(1), key).unwrap();

        assert_eq!(
            keys.key_for(&dest(1)).map(GroupKey::as_slice),
            Some([0xAB; 64].as_slice())
        );
        assert!(keys.key_for(&dest(2)).is_none());
    }

    #[test]
    fn both_aes_128_and_aes_256_key_lengths_store_and_retrieve() {
        let mut keys = TestGroupKeys::default();
        keys.insert(dest(1), GroupKey::from_slice(&[0x11; 32]).unwrap())
            .unwrap();
        keys.insert(dest(2), GroupKey::from_slice(&[0x22; 64]).unwrap())
            .unwrap();

        assert_eq!(
            keys.key_for(&dest(1)).map(GroupKey::as_slice),
            Some([0x11; 32].as_slice())
        );
        assert_eq!(
            keys.key_for(&dest(2)).map(GroupKey::as_slice),
            Some([0x22; 64].as_slice())
        );
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
        assert_eq!(
            keys.key_for(&dest(1)).map(GroupKey::as_slice),
            Some([0x99; 64].as_slice())
        );
    }

    #[test]
    fn a_full_fixed_store_reports_itself_but_still_overwrites_known_destinations() {
        let mut keys = GroupKeys::<FixedGroupKeyTable<2>>::default();
        keys.insert(dest(1), GroupKey::default()).unwrap();
        assert!(keys.has_room());
        keys.insert(dest(2), GroupKey::default()).unwrap();
        assert!(!keys.has_room());

        assert_eq!(
            keys.insert(dest(3), GroupKey::default()),
            Err(TablePushError::TableFull)
        );
        assert_eq!(
            keys.insert(dest(1), GroupKey::from_slice(&[0x77; 32]).unwrap()),
            Ok(())
        );
        assert_eq!(
            keys.key_for(&dest(1)).map(GroupKey::as_slice),
            Some([0x77; 32].as_slice())
        );
    }

    #[test]
    fn heap_columns_track_past_any_fixed_ceiling() {
        let mut keys = GroupKeys::<HeapGroupKeyTable>::default();
        for byte in 0..16u8 {
            keys.insert(dest(byte), GroupKey::from_slice(&[byte; 32]).unwrap())
                .unwrap();
        }
        assert_eq!(keys.len(), 16);
        assert_eq!(
            keys.key_for(&dest(7)).map(GroupKey::as_slice),
            Some([7u8; 32].as_slice())
        );
    }

    fn assert_removal_preserves_alignment<C: GroupKeyTable + Default>() {
        let mut keys = GroupKeys::<C>::default();
        keys.insert(dest(1), GroupKey::from_slice(&[0x11; 32]).unwrap())
            .unwrap();
        keys.insert(dest(2), GroupKey::from_slice(&[0x22; 64]).unwrap())
            .unwrap();

        keys.remove(&dest(1));
        keys.remove(&dest(1));

        assert!(keys.key_for(&dest(1)).is_none());
        assert_eq!(
            keys.key_for(&dest(2)).map(GroupKey::as_slice),
            Some([0x22; 64].as_slice())
        );
        assert_eq!(keys.len(), 1);
        keys.insert(dest(3), GroupKey::from_slice(&[0x33; 32]).unwrap())
            .unwrap();
        assert_eq!(
            keys.key_for(&dest(3)).map(GroupKey::as_slice),
            Some([0x33; 32].as_slice())
        );
    }

    #[test]
    fn removal_preserves_fixed_and_heap_column_alignment() {
        assert_removal_preserves_alignment::<FixedGroupKeyTable<2>>();
        assert_removal_preserves_alignment::<HeapGroupKeyTable>();
    }
}
