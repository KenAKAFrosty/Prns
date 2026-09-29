use crate::routing::links::resources::assembly::{
    AssemblyBytes, AssemblyCorrelation, IncomingAssemblyTable,
};
use crate::routing::links::resources::{ResourceHash, RESOURCE_HASH_LEN};
use crate::routing::links::LinkId;

#[derive(Debug)]
pub struct FixedIncomingAssemblyTable<const MAX_INCOMING_ASSEMBLIES: usize> {
    len: usize,
    link_ids: [LinkId; MAX_INCOMING_ASSEMBLIES],
    original_hashes: [ResourceHash; MAX_INCOMING_ASSEMBLIES],
    correlations: [AssemblyCorrelation; MAX_INCOMING_ASSEMBLIES],
    total_segments: [u64; MAX_INCOMING_ASSEMBLIES],
    stream_sizes: [u64; MAX_INCOMING_ASSEMBLIES],
    segments_received: [u64; MAX_INCOMING_ASSEMBLIES],
    received_totals: [u64; MAX_INCOMING_ASSEMBLIES],
    value_totals: [u64; MAX_INCOMING_ASSEMBLIES],
}

impl<const MAX_INCOMING_ASSEMBLIES: usize> Default
    for FixedIncomingAssemblyTable<MAX_INCOMING_ASSEMBLIES>
{
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); MAX_INCOMING_ASSEMBLIES],
            original_hashes: [ResourceHash::new([0u8; RESOURCE_HASH_LEN]); MAX_INCOMING_ASSEMBLIES],
            correlations: [AssemblyCorrelation::Unsolicited; MAX_INCOMING_ASSEMBLIES],
            total_segments: [0; MAX_INCOMING_ASSEMBLIES],
            stream_sizes: [0; MAX_INCOMING_ASSEMBLIES],
            segments_received: [0; MAX_INCOMING_ASSEMBLIES],
            received_totals: [0; MAX_INCOMING_ASSEMBLIES],
            value_totals: [0; MAX_INCOMING_ASSEMBLIES],
        }
    }
}

impl<const MAX_INCOMING_ASSEMBLIES: usize> IncomingAssemblyTable
    for FixedIncomingAssemblyTable<MAX_INCOMING_ASSEMBLIES>
{
    fn capacity(&self) -> usize {
        MAX_INCOMING_ASSEMBLIES
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
    fn correlations(&self) -> &[AssemblyCorrelation] {
        &self.correlations[..self.len]
    }
    fn total_segments(&self) -> &[u64] {
        &self.total_segments[..self.len]
    }
    fn segments_received(&self) -> &[u64] {
        &self.segments_received[..self.len]
    }
    fn stream_sizes(&self) -> &[u64] {
        &self.stream_sizes[..self.len]
    }
    fn received_totals(&self) -> &[u64] {
        &self.received_totals[..self.len]
    }

    fn value_totals(&self) -> &[u64] {
        &self.value_totals[..self.len]
    }

    fn push(
        &mut self,
        link_id: LinkId,
        original_hash: ResourceHash,
        total_segments: u64,
        stream_size: u64,
        correlation: AssemblyCorrelation,
    ) {
        if self.len >= MAX_INCOMING_ASSEMBLIES {
            return;
        }
        let i = self.len;
        self.link_ids[i] = link_id;
        self.original_hashes[i] = original_hash;
        self.correlations[i] = correlation;
        self.total_segments[i] = total_segments;
        self.stream_sizes[i] = stream_size;
        self.segments_received[i] = 0;
        self.received_totals[i] = 0;
        self.value_totals[i] = 0;
        self.len += 1;
    }

    fn set_progress(&mut self, index: usize, segments_received: u64, bytes: AssemblyBytes) {
        self.segments_received[index] = segments_received;
        self.received_totals[index] = bytes.stream;
        self.value_totals[index] = bytes.value;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.link_ids[index] = self.link_ids[last];
            self.original_hashes[index] = self.original_hashes[last];
            self.correlations[index] = self.correlations[last];
            self.total_segments[index] = self.total_segments[last];
            self.stream_sizes[index] = self.stream_sizes[last];
            self.segments_received[index] = self.segments_received[last];
            self.received_totals[index] = self.received_totals[last];
            self.value_totals[index] = self.value_totals[last];
        }
        self.len = last;
    }
}
