mod impls;

pub use impls::*;

use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;

pub trait IncomingAssemblyColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn link_ids(&self) -> &[LinkId];
    fn original_hashes(&self) -> &[ResourceHash];
    fn total_segments(&self) -> &[u64];
    fn segments_received(&self) -> &[u64];
    fn received_totals(&self) -> &[u64];

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash, total_segments: u64);
    fn set_progress(&mut self, index: usize, segments_received: u64, received_total: u64);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentFit {
    Expected,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyProgress {
    Assembling,
    Complete { total_size: u64 },
}

#[derive(Debug, Default)]
pub struct IncomingAssemblies<C: IncomingAssemblyColumns> {
    columns: C,
}

impl<C: IncomingAssemblyColumns> IncomingAssemblies<C> {
    /// Open a chain on `link_id`: its first segment has been accepted. Any prior
    /// chain on the link is replaced — a link reassembles one transfer at a time,
    /// the same one-resource-per-link invariant [`IncomingResources`](super::table::IncomingResources) keeps.
    pub fn begin(&mut self, link_id: LinkId, original_hash: ResourceHash, total_segments: u64) {
        if let Some(index) = self.index_of(&link_id) {
            self.columns.swap_remove(index);
        }
        if self.columns.len() < self.columns.capacity() {
            self.columns.push(link_id, original_hash, total_segments);
        }
    }

    pub fn fit(
        &self,
        link_id: &LinkId,
        original_hash: &ResourceHash,
        segment_index: u64,
    ) -> SegmentFit {
        let matches = self.index_of(link_id).is_some_and(|index| {
            self.columns.original_hashes()[index] == *original_hash
                && segment_index == self.columns.segments_received()[index] + 1
        });
        if matches {
            SegmentFit::Expected
        } else {
            SegmentFit::Unexpected
        }
    }

    pub fn advance(&mut self, link_id: &LinkId, segment_bytes: u64) -> Option<AssemblyProgress> {
        let index = self.index_of(link_id)?;
        let segments_received = self.columns.segments_received()[index] + 1;
        let received_total = self.columns.received_totals()[index].saturating_add(segment_bytes);
        self.columns
            .set_progress(index, segments_received, received_total);
        if segments_received >= self.columns.total_segments()[index] {
            Some(AssemblyProgress::Complete {
                total_size: received_total,
            })
        } else {
            Some(AssemblyProgress::Assembling)
        }
    }

    pub fn original_hash(&self, link_id: &LinkId) -> Option<ResourceHash> {
        self.index_of(link_id)
            .map(|index| self.columns.original_hashes()[index])
    }

    pub fn clear(&mut self, link_id: &LinkId) {
        if let Some(index) = self.index_of(link_id) {
            self.columns.swap_remove(index);
        }
    }

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.columns
            .link_ids()
            .iter()
            .position(|candidate| candidate == link_id)
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

    fn table() -> IncomingAssemblies<FixedIncomingAssemblyColumns<4>> {
        IncomingAssemblies::default()
    }

    #[test]
    fn advance_assembles_until_the_last_segment_completes() {
        let mut assemblies = table();
        assemblies.begin(link(1), hash(0xA), 3);
        assert_eq!(
            assemblies.advance(&link(1), 100),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            assemblies.advance(&link(1), 100),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            assemblies.advance(&link(1), 50),
            Some(AssemblyProgress::Complete { total_size: 250 })
        );
    }

    #[test]
    fn fit_expects_the_next_segment_of_the_right_chain() {
        let mut assemblies = table();
        assemblies.begin(link(1), hash(0xA), 3);
        assemblies.advance(&link(1), 100);
        assert_eq!(
            assemblies.fit(&link(1), &hash(0xA), 2),
            SegmentFit::Expected
        );
        assert_eq!(
            assemblies.fit(&link(1), &hash(0xA), 3),
            SegmentFit::Unexpected
        );
        assert_eq!(
            assemblies.fit(&link(1), &hash(0xB), 2),
            SegmentFit::Unexpected
        );
        assert_eq!(
            assemblies.fit(&link(2), &hash(0xA), 2),
            SegmentFit::Unexpected
        );
    }

    #[test]
    fn clear_retires_the_chain() {
        let mut assemblies = table();
        assemblies.begin(link(1), hash(0xA), 3);
        assemblies.clear(&link(1));
        assert_eq!(assemblies.advance(&link(1), 100), None);
        assert_eq!(assemblies.original_hash(&link(1)), None);
    }

    #[test]
    fn begin_replaces_a_prior_chain_on_the_same_link() {
        let mut assemblies = table();
        assemblies.begin(link(1), hash(0xA), 2);
        assemblies.begin(link(1), hash(0xB), 3);
        assert_eq!(assemblies.original_hash(&link(1)), Some(hash(0xB)));
    }
}
