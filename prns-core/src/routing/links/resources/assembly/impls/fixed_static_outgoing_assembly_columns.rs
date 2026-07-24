use crate::routing::links::resources::assembly::{
    OutgoingAssemblyTable, StaticResponseContinuation,
};
use crate::routing::links::resources::{ResourceHash, RESOURCE_HASH_LEN};
use crate::routing::links::LinkId;

/// Fixed outgoing assembly rows with the extra continuation state needed by
/// source-capable embedded builds.
///
/// Constrained layouts use `FixedOutgoingAssemblyTable` instead, so they keep
/// the same assembly-row memory footprint as before static file serving.
#[derive(Debug)]
pub struct FixedStaticOutgoingAssemblyTable<const MAX_OUTGOING_ASSEMBLIES: usize> {
    len: usize,
    link_ids: [LinkId; MAX_OUTGOING_ASSEMBLIES],
    original_hashes: [ResourceHash; MAX_OUTGOING_ASSEMBLIES],
    static_continuations: [Option<StaticResponseContinuation>; MAX_OUTGOING_ASSEMBLIES],
}

impl<const MAX_OUTGOING_ASSEMBLIES: usize> Default
    for FixedStaticOutgoingAssemblyTable<MAX_OUTGOING_ASSEMBLIES>
{
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); MAX_OUTGOING_ASSEMBLIES],
            original_hashes: [ResourceHash::new([0u8; RESOURCE_HASH_LEN]); MAX_OUTGOING_ASSEMBLIES],
            static_continuations: [None; MAX_OUTGOING_ASSEMBLIES],
        }
    }
}

impl<const MAX_OUTGOING_ASSEMBLIES: usize> OutgoingAssemblyTable
    for FixedStaticOutgoingAssemblyTable<MAX_OUTGOING_ASSEMBLIES>
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

    fn supports_static_continuations(&self) -> bool {
        true
    }

    fn static_continuation(&self, index: usize) -> Option<StaticResponseContinuation> {
        self.static_continuations[index]
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash) {
        if self.len >= MAX_OUTGOING_ASSEMBLIES {
            return;
        }
        let index = self.len;
        self.link_ids[index] = link_id;
        self.original_hashes[index] = original_hash;
        self.static_continuations[index] = None;
        self.len += 1;
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
        let last = self.len - 1;
        if index != last {
            self.link_ids[index] = self.link_ids[last];
            self.original_hashes[index] = self.original_hashes[last];
            self.static_continuations[index] = self.static_continuations[last];
        }
        self.static_continuations[last] = None;
        self.len = last;
    }
}
