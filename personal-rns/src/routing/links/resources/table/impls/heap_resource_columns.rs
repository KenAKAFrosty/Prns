use alloc::vec;
use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::resources::table::{
    ResourceBuffers, ResourceColumns, ResourceTablePushError,
};
use crate::routing::links::resources::{
    max_part_count, sealed_transfer_len, ResourceHash, MAP_HASH_LEN, MAX_EFFICIENT_SIZE,
};
use crate::routing::links::LinkId;

/// A host-grade ceiling on concurrent transfers across all links — each
/// active slot materializes its full transfer buffer, so this bounds worst
/// -case memory at roughly a mebibyte per slot.
pub const DEFAULT_MAX_RESOURCES: usize = 64;

/// Heap columns for a std host: every slot can hold the largest transfer the
/// protocol allows (a sealed [`MAX_EFFICIENT_SIZE`] stream), and its buffers
/// are allocated when the slot is pushed and freed when it is removed.
#[derive(Debug, Default)]
pub struct HeapResourceColumns<State> {
    link_ids: Vec<LinkId>,
    hashes: Vec<ResourceHash>,
    timeout_ats: Vec<Option<InstantMillis>>,
    states: Vec<State>,
    transfers: Vec<Vec<u8>>,
    part_names: Vec<Vec<[u8; MAP_HASH_LEN]>>,
    part_flags: Vec<Vec<bool>>,
}

const HEAP_TRANSFER_CAPACITY: usize = sealed_transfer_len(MAX_EFFICIENT_SIZE);

impl<State: Default> ResourceColumns<State> for HeapResourceColumns<State> {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_RESOURCES
    }
    fn transfer_capacity(&self) -> usize {
        HEAP_TRANSFER_CAPACITY
    }
    fn part_capacity(&self) -> usize {
        max_part_count(HEAP_TRANSFER_CAPACITY)
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
        self.link_ids.push(link_id);
        self.hashes.push(hash);
        self.timeout_ats.push(None);
        self.states.push(state);
        self.transfers.push(vec![0u8; HEAP_TRANSFER_CAPACITY]);
        self.part_names
            .push(vec![[0u8; MAP_HASH_LEN]; max_part_count(HEAP_TRANSFER_CAPACITY)]);
        self.part_flags
            .push(vec![false; max_part_count(HEAP_TRANSFER_CAPACITY)]);
        Ok(self.link_ids.len() - 1)
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.hashes.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.states.swap_remove(index);
        self.transfers.swap_remove(index);
        self.part_names.swap_remove(index);
        self.part_flags.swap_remove(index);
    }
}
