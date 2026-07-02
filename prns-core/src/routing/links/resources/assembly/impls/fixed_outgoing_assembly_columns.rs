use crate::routing::links::resources::assembly::OutgoingAssemblyColumns;
use crate::routing::links::resources::{ResourceHash, RESOURCE_HASH_LEN};
use crate::routing::links::LinkId;

#[derive(Debug)]
pub struct FixedOutgoingAssemblyColumns<const MAX_OUTGOING_ASSEMBLIES: usize> {
    len: usize,
    link_ids: [LinkId; MAX_OUTGOING_ASSEMBLIES],
    original_hashes: [ResourceHash; MAX_OUTGOING_ASSEMBLIES],
}

impl<const MAX_OUTGOING_ASSEMBLIES: usize> Default
    for FixedOutgoingAssemblyColumns<MAX_OUTGOING_ASSEMBLIES>
{
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); MAX_OUTGOING_ASSEMBLIES],
            original_hashes: [ResourceHash::new([0u8; RESOURCE_HASH_LEN]); MAX_OUTGOING_ASSEMBLIES],
        }
    }
}

impl<const MAX_OUTGOING_ASSEMBLIES: usize> OutgoingAssemblyColumns
    for FixedOutgoingAssemblyColumns<MAX_OUTGOING_ASSEMBLIES>
{
    fn capacity(&self) -> usize {
        MAX_OUTGOING_ASSEMBLIES
    }
    fn len(&self) -> usize {
        self.len
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids[..self.len]
    }
    fn original_hashes(&self) -> &[ResourceHash] {
        &self.original_hashes[..self.len]
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash) {
        if self.len >= MAX_OUTGOING_ASSEMBLIES {
            return;
        }
        let i = self.len;
        self.link_ids[i] = link_id;
        self.original_hashes[i] = original_hash;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.link_ids[index] = self.link_ids[last];
            self.original_hashes[index] = self.original_hashes[last];
        }
        self.len = last;
    }
}
