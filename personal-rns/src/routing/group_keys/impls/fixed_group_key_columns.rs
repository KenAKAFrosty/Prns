use crate::routing::group_keys::{GroupKey, GroupKeyColumns};
use crate::routing::storage::ColumnsFull;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedGroupKeyColumns<const MAX_GROUP_KEYS: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_GROUP_KEYS],
    keys: [GroupKey; MAX_GROUP_KEYS],
}

impl<const MAX_GROUP_KEYS: usize> Default for FixedGroupKeyColumns<MAX_GROUP_KEYS> {
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_GROUP_KEYS],
            keys: core::array::from_fn(|_| GroupKey::default()),
        }
    }
}

impl<const MAX_GROUP_KEYS: usize> GroupKeyColumns for FixedGroupKeyColumns<MAX_GROUP_KEYS> {
    fn capacity(&self) -> usize {
        MAX_GROUP_KEYS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn keys(&self) -> &[GroupKey] {
        &self.keys[..self.len]
    }

    fn upsert(&mut self, destination: DestinationHash, key: GroupKey) -> Result<(), ColumnsFull> {
        if let Some(slot) = self.destinations[..self.len]
            .iter()
            .position(|candidate| *candidate == destination)
        {
            self.keys[slot] = key;
            return Ok(());
        }
        if self.len >= MAX_GROUP_KEYS {
            return Err(ColumnsFull);
        }
        self.destinations[self.len] = destination;
        self.keys[self.len] = key;
        self.len += 1;
        Ok(())
    }
}
