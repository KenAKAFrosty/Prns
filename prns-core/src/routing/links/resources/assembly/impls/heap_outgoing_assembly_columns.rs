use alloc::vec::Vec;

use crate::routing::links::resources::assembly::{
    OutgoingAssemblyTable, StaticResponseContinuation,
};
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
pub struct HeapOutgoingAssemblyTable {
    link_ids: Vec<LinkId>,
    original_hashes: Vec<ResourceHash>,
    static_continuations: Vec<Option<StaticResponseContinuation>>,
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

    fn supports_static_continuations(&self) -> bool {
        true
    }

    fn static_continuation(&self, index: usize) -> Option<StaticResponseContinuation> {
        self.static_continuations[index]
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash) {
        self.link_ids.push(link_id);
        self.original_hashes.push(original_hash);
        self.static_continuations.push(None);
    }

    fn set_static_continuation(
        &mut self,
        index: usize,
        continuation: StaticResponseContinuation,
    ) -> bool {
        self.static_continuations[index] = Some(continuation);
        true
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.original_hashes.swap_remove(index);
        self.static_continuations.swap_remove(index);
    }
}
