use super::*;
use crate::routing::links::resources::{ResourceHash, ResourceSegment};
use crate::routing::links::LinkId;

fn check_size<C: IncomingAssemblyTable + Default>(original: u64, offered: u64) {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let link = LinkId::new([1; 16]);
    let hash = ResourceHash::new([2; 32]);
    let correlation = AssemblyCorrelation::Unsolicited;
    assemblies.begin(link, hash, 2, original, correlation);
    let first = ResourceSegment {
        index: 1,
        total_segments: 2,
        total_data_bytes: original,
    };
    assert_eq!(
        assemblies.advance(
            &link,
            &hash,
            first,
            AssemblyBytes {
                stream: 0,
                value: 0
            },
            correlation
        ),
        Some(AssemblyProgress::Assembling)
    );
    let next = ResourceSegment {
        index: 2,
        total_segments: 2,
        total_data_bytes: offered,
    };
    let expected = if original == offered {
        SegmentFit::Expected
    } else {
        SegmentFit::Unexpected
    };
    assert_eq!(assemblies.fit(&link, &hash, next, correlation), expected);
    let advanced = assemblies.advance(
        &link,
        &hash,
        next,
        AssemblyBytes {
            stream: original,
            value: original,
        },
        correlation,
    );
    assert_eq!(
        advanced,
        if original == offered {
            Some(AssemblyProgress::Complete {
                total_size_bytes: original,
            })
        } else {
            None
        }
    );
    if original != offered {
        assert_eq!(
            assemblies.fit(
                &link,
                &hash,
                ResourceSegment {
                    total_data_bytes: original,
                    ..next
                },
                correlation
            ),
            SegmentFit::Expected
        );
    }
}

proptest::proptest! {
    #[test]
    fn stream_size_identity_is_required_by_fit_and_advance(original in proptest::prelude::any::<u64>(), offered in proptest::prelude::any::<u64>()) {
        for size in [0, original, offered, u64::MAX] {
            check_size::<FixedIncomingAssemblyTable<2>>(original, size);
            #[cfg(feature = "alloc")]
            check_size::<HeapIncomingAssemblyTable>(original, size);
        }
    }
}

fn check_column_lifecycle<C: IncomingAssemblyTable + Default>() {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let first = LinkId::new([1; 16]);
    let second = LinkId::new([2; 16]);
    let hash = ResourceHash::new([3; 32]);
    let correlation = AssemblyCorrelation::Unsolicited;
    assemblies.begin(first, hash, 2, 128, correlation);
    assemblies.begin(second, hash, 2, 256, correlation);
    assemblies.clear(&first);
    for (link, size, expected) in [
        (first, 128, SegmentFit::Unexpected),
        (second, 256, SegmentFit::Expected),
        (second, 128, SegmentFit::Unexpected),
    ] {
        assert_eq!(
            assemblies.fit(
                &link,
                &hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: size
                },
                correlation
            ),
            expected
        );
    }
    assemblies.begin(first, hash, 2, 0, correlation);
    assemblies.begin(second, hash, 2, u64::MAX, correlation);
    for (link, size) in [(first, 0), (second, u64::MAX)] {
        assert_eq!(
            assemblies.fit(
                &link,
                &hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: size
                },
                correlation
            ),
            SegmentFit::Expected
        );
    }
}

#[test]
fn stream_size_columns_survive_swap_removal_reuse_and_replacement() {
    check_column_lifecycle::<FixedIncomingAssemblyTable<2>>();
    #[cfg(feature = "alloc")]
    check_column_lifecycle::<HeapIncomingAssemblyTable>();
}

fn check_atomic_progress<C: IncomingAssemblyTable + Default>(
    declared: u64,
    received: u64,
    offered: u64,
    segments: u64,
) {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let link = LinkId::new([1; 16]);
    let hash = ResourceHash::new([2; 32]);
    let correlation = AssemblyCorrelation::Unsolicited;
    let received = received.min(declared);
    let segment = ResourceSegment {
        index: 1,
        total_segments: segments,
        total_data_bytes: declared,
    };
    assemblies.begin(link, hash, segments, declared, correlation);
    assert_eq!(
        assemblies.advance(
            &link,
            &hash,
            segment,
            AssemblyBytes {
                stream: received,
                value: received
            },
            correlation
        ),
        Some(AssemblyProgress::Assembling)
    );
    let next = ResourceSegment {
        index: 2,
        ..segment
    };
    let total = u128::from(received) + u128::from(offered);
    let valid = total <= u128::from(declared) && (segments > 2 || total == u128::from(declared));
    let expected = if valid {
        Some(if segments == 2 {
            AssemblyProgress::Complete {
                total_size_bytes: declared,
            }
        } else {
            AssemblyProgress::Assembling
        })
    } else {
        None
    };
    assert_eq!(assemblies.fits_stream_size(&link, next, offered), valid);
    assert_eq!(
        assemblies.advance(
            &link,
            &hash,
            next,
            AssemblyBytes {
                stream: offered,
                value: offered
            },
            correlation
        ),
        expected
    );
    if !valid {
        // The rejected transition must leave both position and byte count intact.
        assert_eq!(
            assemblies.fit(&link, &hash, next, correlation),
            SegmentFit::Expected
        );
        assert!(assemblies.fits_stream_size(&link, next, declared - received));
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                next,
                AssemblyBytes {
                    stream: declared - received,
                    value: declared - received
                },
                correlation
            ),
            Some(if segments == 2 {
                AssemblyProgress::Complete {
                    total_size_bytes: declared,
                }
            } else {
                AssemblyProgress::Assembling
            })
        );
    }
}

#[test]
fn stream_progress_rejects_invalid_sizes_without_mutating_the_chain() {
    for (declared, received, offered) in [
        (256, 128, 127),
        (256, 128, 128),
        (256, 128, 129),
        (u64::MAX, u64::MAX, 1),
        (u64::MAX, u64::MAX, 0),
        (0, 0, 0),
        (0, 0, 1),
    ] {
        for segments in [2, 3] {
            check_atomic_progress::<FixedIncomingAssemblyTable<2>>(
                declared, received, offered, segments,
            );
            #[cfg(feature = "alloc")]
            check_atomic_progress::<HeapIncomingAssemblyTable>(
                declared, received, offered, segments,
            );
        }
    }
}

proptest::proptest! {
    #[test]
    fn stream_progress_validation_and_commit_agree(
        declared in proptest::prelude::any::<u64>(),
        received in proptest::prelude::any::<u64>(),
        offered in proptest::prelude::any::<u64>(),
    ) {
        for segments in [2, 3] {
            for bytes in [0, offered, declared - received.min(declared), u64::MAX] {
                check_atomic_progress::<FixedIncomingAssemblyTable<2>>(declared, received, bytes, segments);
                #[cfg(feature = "alloc")]
                check_atomic_progress::<HeapIncomingAssemblyTable>(declared, received, bytes, segments);
            }
        }
    }
}
