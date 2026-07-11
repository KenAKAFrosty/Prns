use alloc::vec;
use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::resources::table::{
    ResourceBuffers, ResourceTable, ResourceTablePushError,
};
use crate::routing::links::resources::{
    max_part_count, sealed_transfer_len, ResourceHash, MAP_HASH_LEN, MAX_EFFICIENT_SIZE,
};
use crate::routing::links::LinkId;

/// A deliberate bound where RNS 1.3.5 grows `Link.outgoing_resources` and `incoming_resources` without limit:
/// Unlike the row-sized tables that take the unbounded-heap convention, each active slot here materializes its full transfer buffer,
/// so an unbounded table would hand remote peers roughly a mebibyte of allocation per accepted offer.
/// Overflow refuses by name on both faces: `SendResourceRejection::TableFull` going out, `IgnoreReason::CapacityExhausted` coming in.
pub const DEFAULT_MAX_RESOURCES: usize = 64;

/// Heap table for a std host: every active slot can hold the largest transfer
/// the protocol allows (a sealed [`MAX_EFFICIENT_SIZE`] stream), and retired
/// slot buffers are kept for reuse by later transfers.
#[derive(Debug, Default)]
pub struct HeapResourceTable<State> {
    link_ids: Vec<LinkId>,
    hashes: Vec<ResourceHash>,
    timeout_ats: Vec<Option<InstantMillis>>,
    states: Vec<State>,
    transfers: Vec<Vec<u8>>,
    part_names: Vec<Vec<[u8; MAP_HASH_LEN]>>,
    part_flags: Vec<Vec<bool>>,
    free_transfers: Vec<Vec<u8>>,
    free_part_names: Vec<Vec<[u8; MAP_HASH_LEN]>>,
    free_part_flags: Vec<Vec<bool>>,
}

const HEAP_TRANSFER_CAPACITY: usize = sealed_transfer_len(MAX_EFFICIENT_SIZE);
const HEAP_PART_CAPACITY: usize = max_part_count(HEAP_TRANSFER_CAPACITY);

impl<State> HeapResourceTable<State> {
    fn take_transfer(&mut self) -> Vec<u8> {
        self.free_transfers
            .pop()
            .unwrap_or_else(|| vec![0u8; HEAP_TRANSFER_CAPACITY])
    }

    fn take_part_names(&mut self) -> Vec<[u8; MAP_HASH_LEN]> {
        self.free_part_names
            .pop()
            .unwrap_or_else(|| vec![[0u8; MAP_HASH_LEN]; HEAP_PART_CAPACITY])
    }

    fn take_part_flags(&mut self) -> Vec<bool> {
        let mut flags = self
            .free_part_flags
            .pop()
            .unwrap_or_else(|| vec![false; HEAP_PART_CAPACITY]);
        flags.fill(false);
        flags
    }
}

impl<State: Default> ResourceTable<State> for HeapResourceTable<State> {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_RESOURCES
    }
    fn transfer_capacity(&self) -> usize {
        HEAP_TRANSFER_CAPACITY
    }
    fn part_capacity(&self) -> usize {
        HEAP_PART_CAPACITY
    }
    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids
    }
    fn hashes(&self) -> &[ResourceHash] {
        &self.hashes
    }
    fn timeout_ats(&self) -> &[Option<InstantMillis>] {
        &self.timeout_ats
    }
    fn states(&self) -> &[State] {
        &self.states
    }

    fn set_hash(&mut self, index: usize, hash: ResourceHash) {
        self.hashes[index] = hash;
    }
    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.timeout_ats[index] = timeout_at;
    }
    fn state_mut(&mut self, index: usize) -> &mut State {
        &mut self.states[index]
    }

    fn transfer(&self, index: usize) -> &[u8] {
        &self.transfers[index]
    }
    fn part_names(&self, index: usize) -> &[[u8; MAP_HASH_LEN]] {
        &self.part_names[index]
    }
    fn part_flags(&self, index: usize) -> &[bool] {
        &self.part_flags[index]
    }
    fn buffers_mut(&mut self, index: usize) -> ResourceBuffers<'_> {
        ResourceBuffers {
            transfer: &mut self.transfers[index],
            part_names: &mut self.part_names[index],
            part_flags: &mut self.part_flags[index],
        }
    }

    fn push(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        state: State,
    ) -> Result<usize, ResourceTablePushError> {
        if self.len() >= self.capacity() {
            return Err(ResourceTablePushError::TableFull);
        }
        let transfer = self.take_transfer();
        let part_names = self.take_part_names();
        let part_flags = self.take_part_flags();
        self.link_ids.push(link_id);
        self.hashes.push(hash);
        self.timeout_ats.push(None);
        self.states.push(state);
        self.transfers.push(transfer);
        self.part_names.push(part_names);
        self.part_flags.push(part_flags);
        Ok(self.link_ids.len() - 1)
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.hashes.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.states.swap_remove(index);
        self.free_transfers.push(self.transfers.swap_remove(index));
        self.free_part_names
            .push(self.part_names.swap_remove(index));
        self.free_part_flags
            .push(self.part_flags.swap_remove(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    #[test]
    fn swap_removed_slot_buffers_are_reused_with_cleared_part_flags() {
        let mut table = HeapResourceTable::<u8>::default();
        let first = table.push(link(1), hash(1), 11).unwrap();
        let transfer = table.transfer(first).as_ptr();
        let names = table.part_names(first).as_ptr();
        let flags = table.part_flags(first).as_ptr();
        table.buffers_mut(first).part_flags[0] = true;

        table.swap_remove(first);
        let second = table.push(link(2), hash(2), 22).unwrap();

        assert_eq!(table.transfer(second).as_ptr(), transfer);
        assert_eq!(table.part_names(second).as_ptr(), names);
        assert_eq!(table.part_flags(second).as_ptr(), flags);
        assert!(!table.part_flags(second).iter().any(|flag| *flag));
        assert_eq!(table.states(), &[22]);
    }
}
