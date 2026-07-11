use alloc::vec::Vec;

use crate::routing::group_keys::{GroupKey, GroupKeyTable};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

pub const DEFAULT_MAX_GROUP_KEYS: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapGroupKeyTable {
    destinations: Vec<DestinationHash>,
    keys: Vec<GroupKey>,
}

impl GroupKeyTable for HeapGroupKeyTable {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_GROUP_KEYS
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn keys(&self) -> &[GroupKey] {
        &self.keys
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        key: GroupKey,
    ) -> Result<(), TablePushError> {
        if let Some(slot) = self
            .destinations
            .iter()
            .position(|candidate| *candidate == destination)
        {
            self.keys[slot] = key;
            return Ok(());
        }
        if self.destinations.len() >= DEFAULT_MAX_GROUP_KEYS {
            return Err(TablePushError::TableFull);
        }
        self.destinations.push(destination);
        self.keys.push(key);
        Ok(())
    }
}
