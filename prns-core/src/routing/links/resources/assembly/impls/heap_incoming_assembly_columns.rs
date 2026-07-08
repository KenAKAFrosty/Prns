use alloc::vec::Vec;

use crate::routing::links::resources::assembly::IncomingAssemblyColumns;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
pub struct HeapIncomingAssemblyColumns {
    link_ids: Vec<LinkId>,
    original_hashes: Vec<ResourceHash>,
    total_segments: Vec<u64>,
    segments_received: Vec<u64>,
    received_totals: Vec<u64>,
}

impl IncomingAssemblyColumns for HeapIncomingAssemblyColumns {
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
    fn total_segments(&self) -> &[u64] {
        &self.total_segments
    }
    fn segments_received(&self) -> &[u64] {
        &self.segments_received
    }
    fn received_totals(&self) -> &[u64] {
        &self.received_totals
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash, total_segments: u64) {
        self.link_ids.push(link_id);
        self.original_hashes.push(original_hash);
        self.total_segments.push(total_segments);
        self.segments_received.push(0);
        self.received_totals.push(0);
    }

    fn set_progress(&mut self, index: usize, segments_received: u64, received_total: u64) {
        self.segments_received[index] = segments_received;
        self.received_totals[index] = received_total;
    }

    fn swap_remove(&mut self, index: usize) {
        self.link_ids.swap_remove(index);
        self.original_hashes.swap_remove(index);
        self.total_segments.swap_remove(index);
        self.segments_received.swap_remove(index);
        self.received_totals.swap_remove(index);
    }
}
