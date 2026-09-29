use super::AssemblyCorrelation;
use crate::engine::CommandId;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceHash, ResourceSegment};
use crate::routing::links::LinkId;
use crate::units::ByteLimit;

/// Verified stream bytes include framing; value bytes count only the delivered body.
#[derive(Debug)]
pub struct AssemblyBytes {
    pub stream: u64,
    pub value: u64,
}

pub trait IncomingAssemblyTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn link_ids(&self) -> &[LinkId];
    fn original_hashes(&self) -> &[ResourceHash];
    fn correlations(&self) -> &[AssemblyCorrelation];
    fn total_segments(&self) -> &[u64];
    fn stream_sizes(&self) -> &[u64];
    fn segments_received(&self) -> &[u64];
    fn received_totals(&self) -> &[u64];
    fn value_totals(&self) -> &[u64];

    fn push(
        &mut self,
        link_id: LinkId,
        original_hash: ResourceHash,
        total_segments: u64,
        stream_size: u64,
        correlation: AssemblyCorrelation,
    );
    fn set_progress(&mut self, index: usize, segments_received: u64, bytes: AssemblyBytes);
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
    Complete { total_size_bytes: u64 },
}

#[derive(Debug, Default)]
pub struct IncomingAssemblies<C: IncomingAssemblyTable> {
    table: C,
}

impl<C: IncomingAssemblyTable> IncomingAssemblies<C> {
    /// Open a chain on `link_id`, replacing any prior chain. Assembly tracking
    /// retains one split chain per link, independently of admitted transfer rows.
    pub fn begin(
        &mut self,
        link_id: LinkId,
        original_hash: ResourceHash,
        total_segments: u64,
        stream_size: u64,
        correlation: AssemblyCorrelation,
    ) {
        if let Some(index) = self.index_of(&link_id) {
            self.table.swap_remove(index);
        }
        if self.table.len() < self.table.capacity() {
            self.table.push(
                link_id,
                original_hash,
                total_segments,
                stream_size,
                correlation,
            );
        }
    }

    /// A continuation must preserve the original count and name the next segment
    /// of the live chain. A completed chain cannot admit another segment.
    pub fn fit(
        &self,
        link_id: &LinkId,
        original_hash: &ResourceHash,
        segment: ResourceSegment,
        correlation: AssemblyCorrelation,
    ) -> SegmentFit {
        let matches = self.index_of(link_id).is_some_and(|index| {
            self.table.original_hashes()[index] == *original_hash
                && self.table.correlations()[index] == correlation
                && segment.total_segments == self.table.total_segments()[index]
                && segment.total_data_bytes == self.table.stream_sizes()[index]
                && segment.index <= segment.total_segments
                && Some(segment.index) == self.table.segments_received()[index].checked_add(1)
        });
        if matches {
            SegmentFit::Expected
        } else {
            SegmentFit::Unexpected
        }
    }

    /// Commit only the next segment of the named chain with a valid cumulative
    /// stream size. A refused transition leaves all progress unchanged.
    pub fn advance(
        &mut self,
        link_id: &LinkId,
        original_hash: &ResourceHash,
        segment: ResourceSegment,
        segment_bytes: AssemblyBytes,
        correlation: AssemblyCorrelation,
    ) -> Option<AssemblyProgress> {
        if self.fit(link_id, original_hash, segment, correlation) == SegmentFit::Unexpected {
            return None;
        }
        let index = self.index_of(link_id)?;
        let segments_received = segment.index;
        let received_total = self.next_stream_total(index, segment, segment_bytes.stream)?;
        if segment_bytes.value > segment_bytes.stream {
            return None;
        }
        let value = self.table.value_totals()[index].checked_add(segment_bytes.value)?;
        self.table.set_progress(
            index,
            segments_received,
            AssemblyBytes {
                stream: received_total,
                value,
            },
        );
        if segments_received >= self.table.total_segments()[index] {
            Some(AssemblyProgress::Complete {
                total_size_bytes: received_total,
            })
        } else {
            Some(AssemblyProgress::Assembling)
        }
    }

    pub fn original_hash(&self, link_id: &LinkId) -> Option<ResourceHash> {
        self.index_of(link_id)
            .map(|index| self.table.original_hashes()[index])
    }

    pub(crate) fn fits_value_limit(&self, link_id: &LinkId, bytes: u64, limit: ByteLimit) -> bool {
        self.index_of(link_id)
            .and_then(|index| self.table.value_totals()[index].checked_add(bytes))
            .is_some_and(|total| limit.allows(total))
    }

    /// Stream bytes include metadata and response framing. Chain identity is
    /// checked separately; this validates the next segment's size declaration.
    pub(crate) fn fits_stream_size(
        &self,
        link_id: &LinkId,
        segment: ResourceSegment,
        segment_bytes: u64,
    ) -> bool {
        let Some(index) = self.index_of(link_id) else {
            return false;
        };
        self.next_stream_total(index, segment, segment_bytes)
            .is_some()
    }

    fn next_stream_total(
        &self,
        index: usize,
        segment: ResourceSegment,
        segment_bytes: u64,
    ) -> Option<u64> {
        let total = self.table.received_totals()[index].checked_add(segment_bytes)?;
        (self.table.total_segments()[index] == segment.total_segments
            && self.table.stream_sizes()[index] == segment.total_data_bytes
            && self.table.segments_received()[index].checked_add(1) == Some(segment.index)
            && segment.index <= segment.total_segments
            && total <= segment.total_data_bytes
            && (segment.index < segment.total_segments || total == segment.total_data_bytes))
            .then_some(total)
    }

    pub fn correlation(&self, link_id: &LinkId) -> Option<AssemblyCorrelation> {
        self.index_of(link_id)
            .map(|index| self.table.correlations()[index])
    }

    pub fn clear(&mut self, link_id: &LinkId) {
        if let Some(index) = self.index_of(link_id) {
            self.table.swap_remove(index);
        }
    }

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.table
            .link_ids()
            .iter()
            .position(|candidate| candidate == link_id)
    }
}

pub trait OutgoingAssemblyTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn link_ids(&self) -> &[LinkId];
    fn original_hashes(&self) -> &[ResourceHash];

    fn supports_static_continuations(&self) -> bool {
        false
    }

    fn static_continuation(&self, _index: usize) -> Option<StaticResponseContinuation> {
        None
    }

    fn set_static_continuation(
        &mut self,
        _index: usize,
        _continuation: StaticResponseContinuation,
    ) -> bool {
        false
    }

    fn push(&mut self, link_id: LinkId, original_hash: ResourceHash);
    fn swap_remove(&mut self, index: usize);
}

/// The flash-backed source for the next automatically segmented response window.
///
/// Only offsets and a static borrow survive between proofs. The live encrypted resource owns at
/// most one segment, so the complete file is never copied into the transfer store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticResponseContinuation {
    pub command_id: CommandId,
    pub request_id: RequestId,
    pub bytes: &'static [u8],
    pub next_offset: usize,
    pub next_segment_index: u64,
    pub total_segments: u64,
    pub total_data_bytes: u64,
    pub metadata_packed_len: u32,
    pub segment_stream_bytes: usize,
}

#[derive(Debug, Default)]
pub struct OutgoingAssemblies<C: OutgoingAssemblyTable> {
    table: C,
}

impl<C: OutgoingAssemblyTable> OutgoingAssemblies<C> {
    /// Record the `original_hash` the chain's first segment minted so every later segment advertises the same one. Any prior chain on the link is replaced (one outgoing transfer per link).
    pub fn begin(&mut self, link_id: LinkId, original_hash: ResourceHash) {
        if let Some(index) = self.index_of(&link_id) {
            self.table.swap_remove(index);
        }
        if self.table.len() < self.table.capacity() {
            self.table.push(link_id, original_hash);
        }
    }

    pub fn supports_static_continuations(&self) -> bool {
        self.table.supports_static_continuations()
    }

    pub fn set_static_continuation(
        &mut self,
        link_id: &LinkId,
        continuation: StaticResponseContinuation,
    ) -> bool {
        let Some(index) = self.index_of(link_id) else {
            return false;
        };
        self.table.set_static_continuation(index, continuation)
    }

    pub fn static_continuation(&self, link_id: &LinkId) -> Option<StaticResponseContinuation> {
        self.index_of(link_id)
            .and_then(|index| self.table.static_continuation(index))
    }

    pub fn original_hash(&self, link_id: &LinkId) -> Option<ResourceHash> {
        self.index_of(link_id)
            .map(|index| self.table.original_hashes()[index])
    }

    pub fn clear(&mut self, link_id: &LinkId) {
        if let Some(index) = self.index_of(link_id) {
            self.table.swap_remove(index);
        }
    }

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.table
            .link_ids()
            .iter()
            .position(|candidate| candidate == link_id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    fn table() -> IncomingAssemblies<FixedIncomingAssemblyTable<4>> {
        IncomingAssemblies::default()
    }

    #[test]
    fn advance_assembles_until_the_last_segment_completes() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            3,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assert_eq!(
            assemblies.advance(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 1,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyBytes {
                    stream: 100,
                    value: 100
                },
                AssemblyCorrelation::Unsolicited
            ),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            assemblies.advance(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 2,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyBytes {
                    stream: 100,
                    value: 100
                },
                AssemblyCorrelation::Unsolicited
            ),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            assemblies.advance(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 3,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyBytes {
                    stream: 800,
                    value: 800
                },
                AssemblyCorrelation::Unsolicited
            ),
            Some(AssemblyProgress::Complete {
                total_size_bytes: 1_000
            })
        );
    }

    #[test]
    fn fit_expects_the_next_segment_of_the_right_chain() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            3,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.advance(
            &link(1),
            &hash(0xA),
            ResourceSegment {
                index: 1,
                total_segments: 3,
                total_data_bytes: 1_000,
            },
            AssemblyBytes {
                stream: 100,
                value: 100,
            },
            AssemblyCorrelation::Unsolicited,
        );
        assert_eq!(
            assemblies.fit(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 2,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Expected
        );
        assert_eq!(
            assemblies.fit(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 3,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Unexpected
        );
        assert_eq!(
            assemblies.fit(
                &link(1),
                &hash(0xB),
                ResourceSegment {
                    index: 2,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Unexpected
        );
        assert_eq!(
            assemblies.fit(
                &link(2),
                &hash(0xA),
                ResourceSegment {
                    index: 2,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Unexpected
        );
    }

    #[test]
    fn fit_rejects_changed_counts_and_segments_after_completion() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            3,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.advance(
            &link(1),
            &hash(0xA),
            ResourceSegment {
                index: 1,
                total_segments: 3,
                total_data_bytes: 1_000,
            },
            AssemblyBytes {
                stream: 100,
                value: 100,
            },
            AssemblyCorrelation::Unsolicited,
        );
        for total in [0, 1, 2, 4, u64::MAX] {
            assert_eq!(
                assemblies.fit(
                    &link(1),
                    &hash(0xA),
                    ResourceSegment {
                        index: 2,
                        total_segments: total,
                        total_data_bytes: 1_000
                    },
                    AssemblyCorrelation::Unsolicited
                ),
                SegmentFit::Unexpected
            );
        }
        assemblies.advance(
            &link(1),
            &hash(0xA),
            ResourceSegment {
                index: 2,
                total_segments: 3,
                total_data_bytes: 1_000,
            },
            AssemblyBytes {
                stream: 100,
                value: 100,
            },
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.advance(
            &link(1),
            &hash(0xA),
            ResourceSegment {
                index: 3,
                total_segments: 3,
                total_data_bytes: 1_000,
            },
            AssemblyBytes {
                stream: 800,
                value: 800,
            },
            AssemblyCorrelation::Unsolicited,
        );
        assert_eq!(
            assemblies.fit(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 4,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Unexpected
        );
    }

    proptest::proptest! {
        #[test]
        fn fit_matches_the_complete_chain_position(
            total in 2u64..=u64::MAX,
            received in proptest::prelude::any::<u64>(),
            offered_total in proptest::prelude::any::<u64>(),
        ) {
            let mut assemblies = table();
            assemblies.begin(link(1), hash(0xA), total, 1_000, AssemblyCorrelation::Unsolicited);
            assemblies.table.set_progress(0, received, AssemblyBytes { stream: 0, value: 0 });
            let next = received.wrapping_add(1);
            for offered in [total, offered_total] {
                let expected = if offered == total && received < total {
                    SegmentFit::Expected
                } else {
                    SegmentFit::Unexpected
                };
                proptest::prop_assert_eq!(
                    assemblies.fit(&link(1), &hash(0xA), ResourceSegment { index: next, total_segments: offered, total_data_bytes: 1_000 }, AssemblyCorrelation::Unsolicited),
                    expected
                );
            }
        }
    }

    #[test]
    fn fit_does_not_wrap_the_segment_index_at_the_integer_limit() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            u64::MAX,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.table.set_progress(
            0,
            u64::MAX,
            AssemblyBytes {
                stream: 0,
                value: 0,
            },
        );
        assert_eq!(
            assemblies.fit(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 0,
                    total_segments: u64::MAX,
                    total_data_bytes: 1_000
                },
                AssemblyCorrelation::Unsolicited
            ),
            SegmentFit::Unexpected
        );
    }

    proptest::proptest! {
        #[test]
        fn advancement_commits_only_the_exact_chain_position(
            total in 2u64..=u64::MAX,
            received in proptest::prelude::any::<u64>(),
            bytes in 0u64..=963,
        ) {
            let next = received.wrapping_add(1);
            for (offered_hash, offered_index, offered_total) in [
                (hash(0xA), next, total),
                (hash(0xB), next, total),
                (hash(0xA), received, total),
                (hash(0xA), next, total - 1),
            ] {
                let mut assemblies = table();
                assemblies.begin(link(1), hash(0xA), total, 1_000, AssemblyCorrelation::Unsolicited);
                assemblies.table.set_progress(0, received, AssemblyBytes { stream: 37, value: 37 });
                let received_total = 37u128 + u128::from(bytes);
                let matches = offered_hash == hash(0xA) && offered_index == next
                    && offered_total == total && received < total
                    && received_total <= 1_000 && (next < total || received_total == 1_000);
                let expected = if matches {
                    Some(if next == total {
                        AssemblyProgress::Complete { total_size_bytes: received_total as u64 }
                    } else {
                        AssemblyProgress::Assembling
                    })
                } else { None };
                proptest::prop_assert_eq!(assemblies.advance(&link(1), &offered_hash, ResourceSegment { index: offered_index, total_segments: offered_total, total_data_bytes: 1_000 }, AssemblyBytes { stream: bytes, value: bytes }, AssemblyCorrelation::Unsolicited), expected);
                proptest::prop_assert_eq!(
                    (assemblies.original_hash(&link(1)), assemblies.table.total_segments(),
                        assemblies.table.segments_received(), assemblies.table.received_totals()),
                    (Some(hash(0xA)), &[total][..],
                        &[if matches { next } else { received }][..],
                        &[if matches { received_total as u64 } else { 37 }][..])
                );
            }
        }
    }

    #[test]
    fn clear_retires_the_chain() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            3,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.clear(&link(1));
        assert_eq!(
            assemblies.advance(
                &link(1),
                &hash(0xA),
                ResourceSegment {
                    index: 1,
                    total_segments: 3,
                    total_data_bytes: 1_000
                },
                AssemblyBytes {
                    stream: 100,
                    value: 100
                },
                AssemblyCorrelation::Unsolicited
            ),
            None
        );
        assert_eq!(assemblies.original_hash(&link(1)), None);
    }

    proptest::proptest! {
        #[test]
        fn stream_size_checks_are_overflow_safe_and_do_not_mutate_progress(
            received in proptest::prelude::any::<u64>(),
            bytes in proptest::prelude::any::<u64>(),
            advertised in proptest::prelude::any::<u64>(),
        ) {
            for index in [2, 3] {
                let mut assemblies = table();
                assemblies.begin(link(1), hash(0xA), 3, advertised, AssemblyCorrelation::Unsolicited);
                assemblies.table.set_progress(0, index - 1, AssemblyBytes { stream: received, value: received });
                let segment = ResourceSegment { index, total_segments: 3, total_data_bytes: advertised };
                let total = u128::from(received) + u128::from(bytes);
                let expected = total <= u128::from(advertised) && (index < 3 || total == u128::from(advertised));
                proptest::prop_assert_eq!(assemblies.fits_stream_size(&link(1), segment, bytes), expected);
                proptest::prop_assert!(!assemblies.fits_stream_size(&link(2), segment, bytes));
                proptest::prop_assert_eq!(
                    (assemblies.table.segments_received(), assemblies.table.received_totals()),
                    (&[index - 1][..], &[received][..])
                );
            }
        }
    }

    #[test]
    fn stream_size_refuses_overflow_even_with_the_largest_advertised_total() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            2,
            u64::MAX,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.table.set_progress(
            0,
            1,
            AssemblyBytes {
                stream: u64::MAX,
                value: u64::MAX,
            },
        );
        assert!(!assemblies.fits_stream_size(
            &link(1),
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: u64::MAX,
            },
            1
        ));
        assert!(assemblies.fits_stream_size(
            &link(1),
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: u64::MAX,
            },
            0
        ));
    }

    #[test]
    fn begin_replaces_a_prior_chain_on_the_same_link() {
        let mut assemblies = table();
        assemblies.begin(
            link(1),
            hash(0xA),
            2,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assemblies.begin(
            link(1),
            hash(0xB),
            3,
            1_000,
            AssemblyCorrelation::Unsolicited,
        );
        assert_eq!(assemblies.original_hash(&link(1)), Some(hash(0xB)));
    }
}
