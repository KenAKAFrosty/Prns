use alloc::vec::Vec;

use crate::routing::group_keys::{GroupKey, GroupKeyTable};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapGroupKeyTable {
    destinations: Vec<DestinationHash>,
    keys: Vec<GroupKey>,
}

impl GroupKeyTable for HeapGroupKeyTable {
    fn capacity(&self) -> usize {
        usize::MAX
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

    fn swap_remove(&mut self, index: usize) {
        if index >= self.destinations.len() {
            return;
        }
        self.destinations.swap_remove(index);
        self.keys.swap_remove(index);
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
        self.destinations.push(destination);
        self.keys.push(key);
        Ok(())
    }
}
