use super::*;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::ResourceSegment;
use crate::routing::links::resources::{ResourceCorrelation, ResourceHash};
use crate::routing::links::LinkId;

fn correlations(id: RequestId) -> [AssemblyCorrelation; 3] {
    [
        AssemblyCorrelation::Unsolicited,
        AssemblyCorrelation::Request(id),
        AssemblyCorrelation::Response(id),
    ]
}

fn correlation_matrix<C: IncomingAssemblyTable + Default>(first: RequestId, second: RequestId) {
    let link = LinkId::new([0xA1; 16]);
    let hash = ResourceHash::new([0xB2; 32]);
    for expected in correlations(first) {
        let mut assemblies = IncomingAssemblies::<C>::default();
        assemblies.begin(link, hash, 2, 42, expected);
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 42
                },
                AssemblyBytes {
                    stream: 31,
                    value: 31
                },
                expected
            ),
            Some(AssemblyProgress::Assembling)
        );
        for offered in correlations(first).into_iter().chain(correlations(second)) {
            if offered == expected {
                assert_eq!(
                    assemblies.fit(
                        &link,
                        &hash,
                        ResourceSegment {
                            index: 2,
                            total_segments: 2,
                            total_data_bytes: 42
                        },
                        offered
                    ),
                    SegmentFit::Expected
                );
                continue;
            }
            assert_eq!(
                assemblies.fit(
                    &link,
                    &hash,
                    ResourceSegment {
                        index: 2,
                        total_segments: 2,
                        total_data_bytes: 42
                    },
                    offered
                ),
                SegmentFit::Unexpected
            );
            assert_eq!(
                assemblies.advance(
                    &link,
                    &hash,
                    ResourceSegment {
                        index: 2,
                        total_segments: 2,
                        total_data_bytes: 42
                    },
                    AssemblyBytes {
                        stream: 11,
                        value: 11
                    },
                    offered
                ),
                None
            );
            assert_eq!(
                (
                    assemblies.original_hash(&link),
                    assemblies.correlation(&link)
                ),
                (Some(hash), Some(expected))
            );
        }
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                ResourceSegment {
                    index: 2,
                    total_segments: 2,
                    total_data_bytes: 42
                },
                AssemblyBytes {
                    stream: 11,
                    value: 11
                },
                expected
            ),
            Some(AssemblyProgress::Complete {
                total_size_bytes: 42
            })
        );
    }
}

proptest::proptest! {
    #[test]
    fn correlation_requires_the_entire_id_and_role(
        first in proptest::prelude::any::<[u8; 16]>(),
        second in proptest::prelude::any::<[u8; 16]>(),
    ) {
        correlation_matrix::<FixedIncomingAssemblyTable<2>>(RequestId(first), RequestId(second));
        #[cfg(feature = "alloc")]
        correlation_matrix::<HeapIncomingAssemblyTable>(RequestId(first), RequestId(second));
    }
}

fn column_ownership<C: IncomingAssemblyTable + Default>() {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let first = LinkId::new([1; 16]);
    let second = LinkId::new([2; 16]);
    let hash = ResourceHash::new([0xB2; 32]);
    let request = AssemblyCorrelation::Request(RequestId([3; 16]));
    let response = AssemblyCorrelation::Response(RequestId([4; 16]));
    assemblies.begin(first, hash, 2, 42, request);
    assemblies.begin(second, hash, 2, 42, response);
    assemblies.clear(&first);
    assert_eq!(
        (
            assemblies.correlation(&first),
            assemblies.correlation(&second)
        ),
        (None, Some(response))
    );
    assemblies.begin(first, hash, 2, 42, AssemblyCorrelation::Unsolicited);
    assemblies.begin(second, hash, 2, 42, request);
    assert_eq!(
        (
            assemblies.correlation(&first),
            assemblies.correlation(&second)
        ),
        (Some(AssemblyCorrelation::Unsolicited), Some(request))
    );
    assemblies.clear(&second);
    assert_eq!(
        (
            assemblies.correlation(&first),
            assemblies.correlation(&second)
        ),
        (Some(AssemblyCorrelation::Unsolicited), None)
    );
}

#[test]
fn correlation_columns_follow_swap_removal_reuse_and_replacement() {
    column_ownership::<FixedIncomingAssemblyTable<2>>();
    #[cfg(feature = "alloc")]
    column_ownership::<HeapIncomingAssemblyTable>();
}

#[test]
fn wire_correlation_does_not_retain_outgoing_request_policy() {
    use crate::engine::RequestResponseTimeout;
    use crate::units::ByteLimit;

    let id = RequestId([3; 16]);
    assert_eq!(
        AssemblyCorrelation::from(ResourceCorrelation::Unsolicited),
        AssemblyCorrelation::Unsolicited
    );
    assert_eq!(
        AssemblyCorrelation::from(ResourceCorrelation::Response(id)),
        AssemblyCorrelation::Response(id)
    );
    for limit in [
        ByteLimit::Unlimited,
        ByteLimit::Maximum(0),
        ByteLimit::Maximum(u64::MAX),
    ] {
        assert_eq!(
            AssemblyCorrelation::from(ResourceCorrelation::Request {
                id,
                response_timeout: RequestResponseTimeout::LinkDefault,
                maximum_response_bytes: limit,
            }),
            AssemblyCorrelation::Request(id)
        );
    }
}
