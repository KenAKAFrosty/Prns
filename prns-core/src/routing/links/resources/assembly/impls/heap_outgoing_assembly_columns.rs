use alloc::vec::Vec;

use crate::routing::links::resources::assembly::OutgoingAssemblyTable;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
pub struct HeapOutgoingAssemblyTable {
    link_ids: Vec<LinkId>,
    original_hashes: Vec<ResourceHash>,
}

impl OutgoingAssemblyTable for HeapOutgoingAssemblyTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids
    }
    fn original_hashes(&self) -> &[ResourceHash] {
        &self.original_hashes
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash) {
        self.link_ids.push(link_id);
        self.original_hashes.push(original_hash);
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.original_hashes.swap_remove(index);
    }
}
