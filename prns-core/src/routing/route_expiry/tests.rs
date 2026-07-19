#[cfg(feature = "std")]
use super::{RoaringRouteExpiryIndex, RouteExpiryIndex};
#[cfg(feature = "std")]
use crate::units::InstantMillis;

#[cfg(feature = "std")]
#[test]
fn route_adapter_preserves_exact_queries_and_lazy_rebuilds() {
    let index = RoaringRouteExpiryIndex::default();
    let mut values = [
        InstantMillis(599_000),
        InstantMillis(301_000),
        InstantMillis(300_500),
        InstantMillis(900_000),
    ];
    for (row, expiry) in values.iter().copied().enumerate() {
        index.insert(row, expiry);
    }
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(300_500)),
    );
    values[2] = InstantMillis(1_200_000);
    index.invalidate();
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(301_000)),
    );
}
