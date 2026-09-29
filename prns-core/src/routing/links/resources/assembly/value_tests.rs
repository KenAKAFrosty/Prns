use super::*;
use crate::routing::links::resources::{ResourceHash, ResourceSegment};
use crate::routing::links::LinkId;
use crate::units::ByteLimit;

fn check_budget<C: IncomingAssemblyTable + Default>(
    stream: u64,
    value: u64,
    offered: u64,
    maximum: u64,
) {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let link = LinkId::new([1; 16]);
    let hash = ResourceHash::new([2; 32]);
    let value = value.min(stream);
    let correlation = AssemblyCorrelation::Unsolicited;
    assemblies.begin(link, hash, 2, u64::MAX, correlation);
    assert_eq!(
        assemblies.advance(
            &link,
            &hash,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: u64::MAX
            },
            AssemblyBytes { stream, value },
            correlation
        ),
        Some(AssemblyProgress::Assembling)
    );
    let total = u128::from(value) + u128::from(offered);
    for limit in [
        ByteLimit::Maximum(0),
        ByteLimit::Maximum(maximum),
        ByteLimit::Maximum(u64::MAX),
        ByteLimit::Unlimited,
    ] {
        let ceiling = match limit {
            ByteLimit::Maximum(bytes) => bytes,
            ByteLimit::Unlimited => u64::MAX,
        };
        assert_eq!(
            assemblies.fits_value_limit(&link, offered, limit),
            total <= u128::from(ceiling)
        );
    }
    assert!(assemblies.fits_value_limit(&link, 0, ByteLimit::Maximum(value)));
    if value > 0 {
        assert!(!assemblies.fits_value_limit(&link, 0, ByteLimit::Maximum(value - 1)));
    }
}

proptest::proptest! {
    #[test]
    fn cumulative_value_budget_is_independent_of_stream_framing(
        stream in proptest::prelude::any::<u64>(), value in proptest::prelude::any::<u64>(),
        offered in proptest::prelude::any::<u64>(), maximum in proptest::prelude::any::<u64>(),
    ) {
        check_budget::<FixedIncomingAssemblyTable<2>>(stream, value, offered, maximum);
        #[cfg(feature = "alloc")]
        check_budget::<HeapIncomingAssemblyTable>(stream, value, offered, maximum);
    }
}

fn check_lifecycle<C: IncomingAssemblyTable + Default>() {
    let mut assemblies = IncomingAssemblies::<C>::default();
    let first = LinkId::new([1; 16]);
    let second = LinkId::new([2; 16]);
    let hash = ResourceHash::new([3; 32]);
    let correlation = AssemblyCorrelation::Unsolicited;
    let segment = ResourceSegment {
        index: 1,
        total_segments: 2,
        total_data_bytes: 256,
    };
    for (link, value) in [(first, 7), (second, 31)] {
        assemblies.begin(link, hash, 2, 256, correlation);
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                segment,
                AssemblyBytes { stream: 128, value },
                correlation
            ),
            Some(AssemblyProgress::Assembling)
        );
    }
    assemblies.clear(&first);
    assert!(!assemblies.fits_value_limit(&first, 0, ByteLimit::Unlimited));
    assert!(assemblies.fits_value_limit(&second, 1, ByteLimit::Maximum(32)));
    assert!(!assemblies.fits_value_limit(&second, 1, ByteLimit::Maximum(31)));
    for link in [first, second] {
        assemblies.begin(link, hash, 2, 256, correlation);
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                segment,
                AssemblyBytes {
                    stream: 128,
                    value: 129
                },
                correlation
            ),
            None
        );
        assert!(assemblies.fits_value_limit(&link, 0, ByteLimit::Maximum(0)));
        assert_eq!(
            assemblies.advance(
                &link,
                &hash,
                segment,
                AssemblyBytes {
                    stream: 128,
                    value: 1
                },
                correlation
            ),
            Some(AssemblyProgress::Assembling)
        );
    }
}

#[test]
fn value_accounting_preserves_rows_and_rejects_impossible_deltas_atomically() {
    check_lifecycle::<FixedIncomingAssemblyTable<2>>();
    #[cfg(feature = "alloc")]
    check_lifecycle::<HeapIncomingAssemblyTable>();
    for (stream, value, offered, maximum) in [
        (u64::MAX, u64::MAX, 1, u64::MAX),
        (128, 0, 0, 0),
        (128, 1, 0, 0),
    ] {
        check_budget::<FixedIncomingAssemblyTable<2>>(stream, value, offered, maximum);
        #[cfg(feature = "alloc")]
        check_budget::<HeapIncomingAssemblyTable>(stream, value, offered, maximum);
    }
}
