use crate::routing::group_keys::{GroupKey, GroupKeyTable};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedGroupKeyTable<const MAX_GROUP_KEYS: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_GROUP_KEYS],
    keys: [GroupKey; MAX_GROUP_KEYS],
}

impl<const MAX_GROUP_KEYS: usize> Default for FixedGroupKeyTable<MAX_GROUP_KEYS> {
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_GROUP_KEYS],
            keys: core::array::from_fn(|_| GroupKey::default()),
        }
    }
}

impl<const MAX_GROUP_KEYS: usize> GroupKeyTable for FixedGroupKeyTable<MAX_GROUP_KEYS> {
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

    fn swap_remove(&mut self, index: usize) {
        if index >= self.len {
            return;
        }
        let last = self.len - 1;
        self.destinations.swap(index, last);
        self.keys.swap(index, last);
        self.destinations[last] = DestinationHash::new([0u8; 16]);
        self.keys[last] = GroupKey::default();
        self.len -= 1;
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        key: GroupKey,
    ) -> Result<(), TablePushError> {
        if let Some(slot) = self.destinations[..self.len]
            .iter()
            .position(|candidate| *candidate == destination)
        {
            self.keys[slot] = key;
            return Ok(());
        }
        if self.len >= MAX_GROUP_KEYS {
            return Err(TablePushError::TableFull);
        }
        self.destinations[self.len] = destination;
        self.keys[self.len] = key;
        self.len += 1;
        Ok(())
    }
}
